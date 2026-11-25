//! Pure parsers and allowlists for peer verification. Nothing here touches the OS, so every
//! platform tests the Linux, macOS and Windows formats.
use std::path::{Path, PathBuf};

/// Parent pid and start time (clock ticks since boot) from the text of `/proc/<pid>/stat`.
/// The command name is in parentheses and may itself contain spaces and parentheses, so the
/// fields are read after the last `)`.
pub(crate) fn parse_proc_stat(stat: &str) -> Option<(u32, u64)> {
    let rest = &stat[stat.rfind(')')? + 1..];
    let mut fields = rest.split_whitespace();
    // Field 3 is the state, field 4 the parent pid, field 22 the start time.
    let ppid = fields.nth(1)?.parse().ok()?;
    let start = fields.nth(17)?.parse().ok()?;
    Some((ppid, start))
}

/// The `--user-data-dir` value in a list of arguments (`--user-data-dir=DIR` or two arguments).
/// Chrome uses the last occurrence.
pub(crate) fn user_data_dir_from_args<S: AsRef<str>>(args: &[S]) -> Option<PathBuf> {
    let mut found = None;
    let mut iter = args.iter().map(|a| a.as_ref());
    while let Some(arg) = iter.next() {
        if let Some(v) = arg.strip_prefix("--user-data-dir=") {
            found = Some(PathBuf::from(v));
        } else if arg == "--user-data-dir" {
            if let Some(v) = iter.next() {
                found = Some(PathBuf::from(v));
            }
        }
    }
    found.filter(|p| !p.as_os_str().is_empty())
}

/// The data dir from the NUL-separated bytes of `/proc/<pid>/cmdline`.
pub(crate) fn user_data_dir_from_cmdline(raw: &[u8]) -> Option<PathBuf> {
    let args: Vec<String> = raw
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .map(|s| String::from_utf8_lossy(s).into_owned())
        .collect();
    user_data_dir_from_args(&args)
}

/// The data dir from a macOS `KERN_PROCARGS2` buffer: `argc` (i32 in host order), the executable
/// path, NUL padding, then `argc` NUL-terminated arguments (the environment follows).
pub(crate) fn user_data_dir_from_procargs2(raw: &[u8]) -> Option<PathBuf> {
    let argc = i32::from_ne_bytes(raw.get(..4)?.try_into().ok()?);
    let argc = usize::try_from(argc).ok()?;
    let mut rest = &raw[4..];
    let exe_end = rest.iter().position(|b| *b == 0)?;
    rest = &rest[exe_end..];
    let start = rest.iter().position(|b| *b != 0)?;
    rest = &rest[start..];
    let mut args = Vec::new();
    for part in rest.split(|b| *b == 0).take(argc) {
        args.push(String::from_utf8_lossy(part).into_owned());
    }
    user_data_dir_from_args(&args)
}

/// Linux Google Chrome (stable, beta, unstable) as the packages install it. The path is the
/// resolved `/proc/<pid>/exe` target, so launcher symlinks such as `/usr/bin/google-chrome` are
/// already followed.
pub(crate) fn is_chrome_linux_exe(exe: &Path) -> bool {
    [
        "/opt/google/chrome/chrome",
        "/opt/google/chrome-beta/chrome",
        "/opt/google/chrome-unstable/chrome",
    ]
    .iter()
    .any(|p| exe == Path::new(p))
}

fn windows_norm(path: &str) -> String {
    path.replace('/', "\\").to_ascii_lowercase()
}

/// Windows Google Chrome (stable, beta, canary) wherever it is installed.
pub(crate) fn is_chrome_windows_exe(path: &str) -> bool {
    let p = windows_norm(path);
    [
        "\\google\\chrome\\application\\chrome.exe",
        "\\google\\chrome beta\\application\\chrome.exe",
        "\\google\\chrome sxs\\application\\chrome.exe",
    ]
    .iter()
    .any(|suffix| p.ends_with(suffix))
}

/// Chrome may start the native host through `cmd.exe`; only that shim may sit between the host and
/// the browser.
pub(crate) fn is_windows_shim(path: &str) -> bool {
    windows_norm(path).ends_with("\\windows\\system32\\cmd.exe")
}

/// macOS code requirement for the parent browser: Google's team id and a Chrome signing
/// identifier (stable, beta, dev, canary). v1 accepts Google Chrome only.
pub(crate) const CHROME_REQUIREMENT: &str = concat!(
    "anchor apple generic and certificate leaf[subject.OU] = \"EQHXZ8M8AV\" and (",
    "identifier \"com.google.Chrome\" or identifier \"com.google.Chrome.beta\" or ",
    "identifier \"com.google.Chrome.dev\" or identifier \"com.google.Chrome.canary\")"
);

/// The socket path has to fit `sockaddr_un.sun_path` (104 bytes on macOS, 108 on Linux) with the
/// terminating NUL; 104 is the common limit.
pub(crate) const SUN_PATH_MAX: usize = 104;

/// The Windows pipe name for a user: `\\.\pipe\zptah-browser-<hash of the user SID>`, with a `-dev`
/// suffix for Native Dev. The hash keeps the SID out of the name and the name short.
pub(crate) fn pipe_name_for_sid(sid: &str, dev: bool) -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(sid.as_bytes());
    let hash: String = digest[..8].iter().map(|b| format!("{b:02x}")).collect();
    let suffix = if dev { "-dev" } else { "" };
    format!(r"\\.\pipe\zptah-browser-{hash}{suffix}")
}
