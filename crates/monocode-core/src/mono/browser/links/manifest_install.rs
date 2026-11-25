//! Native messaging host manifests (spec 12.1, 12.2). User-level only, no admin rights. The
//! locations are a table, so a later browser is a new row; v1 writes only the Chrome row.
//!
//! Every function takes an [`InstallEnv`], so tests use a temp directory and a fake registry and
//! never touch the real browser directories. The `path` in a manifest is the signed app binary
//! itself, never a script (spec 12.1).
use super::super::peer::Flavor;
use serde_json::json;
use std::io;
use std::path::{Path, PathBuf};

/// Host name for the release app. Must match `HOST_NAME` in the extension's `config.js`.
pub(crate) const HOST_NAME: &str = "app.zptah.browser_host";
/// Host name Native Dev writes (`DEV_HOST_NAME` in the extension's `scripts/build.mjs`).
pub(crate) const DEV_HOST_NAME: &str = "app.zptah.browser_host_dev";
/// Fixed by the `key` in `extensions/zptah-browser/manifest.dev.json`.
pub(crate) const DEV_EXTENSION_ID: &str = "bmmfejkhajglhijajgdgfckhonddijjh";
/// Chrome Web Store id of zptah Browser. PLACEHOLDER: the listing does not exist yet (spec 13.4).
/// Replace it with the real id when the listing is created; `store_ids_are_valid` keeps the
/// format honest meanwhile. Host mode compiles its own list from the same constant.
pub(crate) const STORE_EXTENSION_IDS: &[&str] = &["pppppppppppppppppppppppppppppppp"];

/// Manifests of other products that trust a public dev id; the app only warns about them
/// (spec 12.1) and never touches them.
const FOREIGN_HOSTS: &[&str] = &["com.agent_browser.connect", "com.leeguoo.chrome_use"];

pub(crate) fn host_name(flavor: Flavor) -> &'static str {
    match flavor {
        Flavor::Release => HOST_NAME,
        Flavor::Dev => DEV_HOST_NAME,
    }
}

/// The extension origins a manifest lists. The release manifest never lists the dev id: its public
/// key is in the repository, so anyone could load an unpacked copy with that id.
pub(crate) fn allowed_origins(flavor: Flavor) -> Vec<String> {
    match flavor {
        Flavor::Release => STORE_EXTENSION_IDS
            .iter()
            .map(|id| format!("chrome-extension://{id}/"))
            .collect(),
        Flavor::Dev => vec![format!("chrome-extension://{DEV_EXTENSION_ID}/")],
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Os {
    Mac,
    Linux,
    Windows,
}

impl Os {
    pub(crate) fn current() -> Os {
        if cfg!(target_os = "macos") {
            Os::Mac
        } else if cfg!(windows) {
            Os::Windows
        } else {
            Os::Linux
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Browser {
    Chrome,
    ChromeBeta,
    ChromeCanary,
    Chromium,
    Edge,
    Brave,
    Arc,
}

/// Browsers v1 writes manifests for (decision 12).
pub(crate) const V1_BROWSERS: &[Browser] = &[Browser::Chrome];

struct Row {
    browser: Browser,
    /// User data dir under `~/Library/Application Support`.
    mac: Option<&'static str>,
    /// User data dir under `~/.config`.
    linux: Option<&'static str>,
    /// User data dir under `%LOCALAPPDATA%`, used to detect the browser.
    win_data: Option<&'static str>,
    /// HKCU key that holds the `NativeMessagingHosts` entries.
    win_key: Option<&'static str>,
}

const CHROME_KEY: &str = r"Software\Google\Chrome\NativeMessagingHosts";

const TABLE: &[Row] = &[
    Row {
        browser: Browser::Chrome,
        mac: Some("Google/Chrome"),
        linux: Some("google-chrome"),
        win_data: Some(r"Google\Chrome\User Data"),
        win_key: Some(CHROME_KEY),
    },
    Row {
        browser: Browser::ChromeBeta,
        mac: Some("Google/Chrome Beta"),
        linux: Some("google-chrome-beta"),
        win_data: Some(r"Google\Chrome Beta\User Data"),
        win_key: Some(CHROME_KEY),
    },
    Row {
        browser: Browser::ChromeCanary,
        mac: Some("Google/Chrome Canary"),
        linux: Some("google-chrome-unstable"),
        win_data: Some(r"Google\Chrome SxS\User Data"),
        win_key: Some(CHROME_KEY),
    },
    Row {
        browser: Browser::Chromium,
        mac: Some("Chromium"),
        linux: Some("chromium"),
        win_data: Some(r"Chromium\User Data"),
        win_key: Some(r"Software\Chromium\NativeMessagingHosts"),
    },
    Row {
        browser: Browser::Edge,
        mac: Some("Microsoft Edge"),
        linux: Some("microsoft-edge"),
        win_data: Some(r"Microsoft\Edge\User Data"),
        win_key: Some(r"Software\Microsoft\Edge\NativeMessagingHosts"),
    },
    Row {
        browser: Browser::Brave,
        mac: Some("BraveSoftware/Brave-Browser"),
        linux: Some("BraveSoftware/Brave-Browser"),
        win_data: Some(r"BraveSoftware\Brave-Browser\User Data"),
        win_key: Some(r"Software\BraveSoftware\Brave-Browser\NativeMessagingHosts"),
    },
    // Unverified (spec 12.2): kept for the table, not written in v1.
    Row {
        browser: Browser::Arc,
        mac: Some("Arc/User Data"),
        linux: None,
        win_data: None,
        win_key: None,
    },
];

fn row(browser: Browser) -> &'static Row {
    TABLE
        .iter()
        .find(|r| r.browser == browser)
        .expect("every browser has a row")
}

/// Where things live on this machine. Tests build one over a temp directory.
#[derive(Debug, Clone)]
pub(crate) struct InstallEnv {
    pub os: Os,
    pub home: PathBuf,
    pub local_app_data: Option<PathBuf>,
    /// Windows only: the manifest file lives here and the registry points at it.
    pub app_data_dir: PathBuf,
}

impl InstallEnv {
    pub(crate) fn from_process(app_data_dir: PathBuf) -> Option<Self> {
        let home = std::env::var_os("HOME")
            .or_else(|| std::env::var_os("USERPROFILE"))
            .map(PathBuf::from)
            .filter(|p| p.is_absolute())?;
        Some(Self {
            os: Os::current(),
            home,
            local_app_data: std::env::var_os("LOCALAPPDATA").map(PathBuf::from),
            app_data_dir,
        })
    }

    /// The browser's user data dir, used to tell whether it is installed.
    fn data_dir(&self, browser: Browser) -> Option<PathBuf> {
        let r = row(browser);
        match self.os {
            Os::Mac => r
                .mac
                .map(|rel| self.home.join("Library/Application Support").join(rel)),
            Os::Linux => r.linux.map(|rel| self.home.join(".config").join(rel)),
            Os::Windows => match (&self.local_app_data, r.win_data) {
                (Some(base), Some(rel)) => Some(base.join(rel)),
                _ => None,
            },
        }
    }
}

/// Where one browser looks for the manifest.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Target {
    /// `<data dir>/NativeMessagingHosts/<host>.json`.
    File(PathBuf),
    /// A manifest file in the app data dir plus the HKCU key whose default value is its path.
    Registry { key: String, file: PathBuf },
}

pub(crate) fn target(env: &InstallEnv, browser: Browser, flavor: Flavor) -> Option<Target> {
    let host = host_name(flavor);
    match env.os {
        Os::Mac | Os::Linux => Some(Target::File(
            env.data_dir(browser)?
                .join("NativeMessagingHosts")
                .join(format!("{host}.json")),
        )),
        Os::Windows => Some(Target::Registry {
            key: format!(r"{}\{host}", row(browser).win_key?),
            file: env
                .app_data_dir
                .join("browser-host")
                .join(format!("{host}.json")),
        }),
    }
}

/// Browsers from `wanted` whose user data dir exists (cheap detection, spec 12.3 step 1).
pub(crate) fn detected(env: &InstallEnv, wanted: &[Browser]) -> Vec<Browser> {
    wanted
        .iter()
        .copied()
        .filter(|b| env.data_dir(*b).map(|d| d.is_dir()).unwrap_or(false))
        .collect()
}

#[derive(Debug)]
pub(crate) enum InstallError {
    /// The executable path is relative, empty or not valid for a manifest.
    BadExecutable(PathBuf),
    /// Only a debug build may write the dev manifest.
    DevNotAllowed,
    /// The destination (or its folder) is a symlink or not a regular file.
    UnsafeDestination(PathBuf),
    Io(io::Error),
}

impl std::fmt::Display for InstallError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstallError::BadExecutable(p) => {
                write!(f, "not an absolute executable path: {}", p.display())
            }
            InstallError::DevNotAllowed => f.write_str("the dev manifest is for debug builds only"),
            InstallError::UnsafeDestination(p) => {
                write!(f, "refusing to write through {}", p.display())
            }
            InstallError::Io(e) => write!(f, "io error: {e}"),
        }
    }
}
impl std::error::Error for InstallError {}
impl From<io::Error> for InstallError {
    fn from(e: io::Error) -> Self {
        InstallError::Io(e)
    }
}

/// The manifest text for `exe`.
pub(crate) fn manifest_json(flavor: Flavor, exe: &Path) -> Result<String, InstallError> {
    if flavor == Flavor::Dev && !cfg!(any(test, debug_assertions)) {
        return Err(InstallError::DevNotAllowed);
    }
    let text = exe.to_str().filter(|t| !t.is_empty() && !t.contains('\0'));
    let Some(text) = text.filter(|_| exe.is_absolute()) else {
        return Err(InstallError::BadExecutable(exe.to_path_buf()));
    };
    let manifest = json!({
        "name": host_name(flavor),
        "description": match flavor {
            Flavor::Release => "zptah Browser host",
            Flavor::Dev => "zptah Browser host (dev)",
        },
        "path": text,
        "type": "stdio",
        "allowed_origins": allowed_origins(flavor),
    });
    let mut out = serde_json::to_string_pretty(&manifest).expect("a json! value serializes");
    out.push('\n');
    Ok(out)
}

/// The Windows registry, injected so tests use a fake.
pub(crate) trait Registry {
    /// Creates `HKCU\<key>` when missing and sets its default value.
    fn set_default(&self, key: &str, value: &str) -> io::Result<()>;
    /// Deletes `HKCU\<key>`; a missing key is not an error.
    fn delete_key(&self, key: &str) -> io::Result<()>;
}

/// The real registry on Windows.
#[cfg(windows)]
pub(crate) struct WindowsRegistry;

#[cfg(windows)]
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(std::iter::once(0)).collect()
}

#[cfg(windows)]
impl Registry for WindowsRegistry {
    fn set_default(&self, key: &str, value: &str) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{RegSetKeyValueW, HKEY_CURRENT_USER, REG_SZ};
        let key = wide(key);
        let data = wide(value);
        // SAFETY: `key` and `data` are NUL-terminated and outlive the call; the byte count covers
        // the data including its terminator.
        let rc = unsafe {
            RegSetKeyValueW(
                HKEY_CURRENT_USER,
                key.as_ptr(),
                std::ptr::null(),
                REG_SZ,
                data.as_ptr().cast(),
                (data.len() * 2) as u32,
            )
        };
        if rc == 0 {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(rc as i32))
        }
    }

    fn delete_key(&self, key: &str) -> io::Result<()> {
        use windows_sys::Win32::System::Registry::{RegDeleteKeyW, HKEY_CURRENT_USER};
        const ERROR_FILE_NOT_FOUND: u32 = 2;
        let key = wide(key);
        // SAFETY: `key` is NUL-terminated and outlives the call.
        let rc = unsafe { RegDeleteKeyW(HKEY_CURRENT_USER, key.as_ptr()) };
        if rc == 0 || rc == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(io::Error::from_raw_os_error(rc as i32))
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub(crate) enum Change {
    Written,
    Unchanged,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct Installed {
    pub browser: Browser,
    pub target: Target,
    pub change: Change,
}

/// Writes the manifest for every detected browser of `wanted`. Idempotent: an identical file or
/// registry value is left alone, a manifest for a moved or updated app is rewritten.
pub(crate) fn install(
    env: &InstallEnv,
    flavor: Flavor,
    exe: &Path,
    wanted: &[Browser],
    registry: &dyn Registry,
) -> Result<Vec<Installed>, InstallError> {
    let text = manifest_json(flavor, exe)?;
    let mut out = Vec::new();
    for browser in detected(env, wanted) {
        let Some(target) = target(env, browser, flavor) else {
            continue;
        };
        let change = match &target {
            Target::File(path) => write_file(path, &text)?,
            Target::Registry { key, file } => {
                let file_change = write_file(file, &text)?;
                registry.set_default(key, &file.to_string_lossy())?;
                file_change
            }
        };
        out.push(Installed {
            browser,
            target,
            change,
        });
    }
    Ok(out)
}

/// Removes the manifests ("Disconnect all browsers"). Browsers need not be detected: a leftover
/// from an uninstalled browser goes too. Only our own host name is touched.
pub(crate) fn remove(
    env: &InstallEnv,
    flavor: Flavor,
    wanted: &[Browser],
    registry: &dyn Registry,
) -> Result<(), InstallError> {
    for browser in wanted {
        match target(env, *browser, flavor) {
            Some(Target::File(path)) => remove_file(&path)?,
            Some(Target::Registry { key, file }) => {
                registry.delete_key(&key)?;
                remove_file(&file)?;
            }
            None => {}
        }
    }
    Ok(())
}

/// Manifests of other products that trust a public dev id (spec 12.1). Reported, never changed.
/// Only unix file locations are searched; Windows keeps them in the registry.
pub(crate) fn foreign_manifests(env: &InstallEnv, wanted: &[Browser]) -> Vec<PathBuf> {
    let mut found = Vec::new();
    for browser in wanted {
        let Some(dir) = env.data_dir(*browser) else {
            continue;
        };
        for host in FOREIGN_HOSTS {
            let path = dir
                .join("NativeMessagingHosts")
                .join(format!("{host}.json"));
            if path.symlink_metadata().is_ok() {
                found.push(path);
            }
        }
    }
    found
}

fn write_file(path: &Path, text: &str) -> Result<Change, InstallError> {
    let dir = path.parent().expect("a manifest path has a parent");
    reject_symlink(dir)?;
    std::fs::create_dir_all(dir)?;
    reject_symlink(dir)?;
    match path.symlink_metadata() {
        Ok(meta) if !meta.file_type().is_file() => {
            return Err(InstallError::UnsafeDestination(path.to_path_buf()))
        }
        Ok(_) => {
            if std::fs::read_to_string(path)
                .map(|old| old == text)
                .unwrap_or(false)
            {
                return Ok(Change::Unchanged);
            }
        }
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    // Same directory, so the rename is atomic and a browser never reads half a manifest.
    let tmp = dir.join(format!(
        ".{}.{:016x}.tmp",
        path.file_name()
            .and_then(|n| n.to_str())
            .unwrap_or("manifest"),
        rand::random::<u64>()
    ));
    // `create_new` (O_EXCL) never follows a planted file or link at the temp name.
    let written = std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&tmp)
        .and_then(|mut f| io::Write::write_all(&mut f, text.as_bytes()));
    if let Err(e) = written {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    if let Err(e) = std::fs::rename(&tmp, path) {
        let _ = std::fs::remove_file(&tmp);
        return Err(e.into());
    }
    Ok(Change::Written)
}

fn remove_file(path: &Path) -> Result<(), InstallError> {
    match path.symlink_metadata() {
        Ok(meta) if meta.file_type().is_file() => std::fs::remove_file(path)?,
        Ok(_) => return Err(InstallError::UnsafeDestination(path.to_path_buf())),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {}
        Err(e) => return Err(e.into()),
    }
    Ok(())
}

fn reject_symlink(dir: &Path) -> Result<(), InstallError> {
    match dir.symlink_metadata() {
        Ok(meta) if meta.file_type().is_symlink() => {
            Err(InstallError::UnsafeDestination(dir.to_path_buf()))
        }
        _ => Ok(()),
    }
}
