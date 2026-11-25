//! Profile names from Chrome's `Local State` (spec 12.4). Reading it is optional: without it the
//! user names the connection themselves.
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) struct ProfileInfo {
    /// The directory under the user data dir, as `--profile-directory` takes it ("Default").
    pub dir: String,
    /// What the user sees in Chrome ("Work").
    pub name: String,
}

/// Profiles listed in `<user_data_dir>/Local State`, sorted by directory name. Empty when the file
/// is missing, unreadable or not the expected shape.
pub(crate) fn read_profiles(user_data_dir: &Path) -> Vec<ProfileInfo> {
    let Ok(bytes) = std::fs::read(user_data_dir.join("Local State")) else {
        return Vec::new();
    };
    parse_local_state(&bytes)
}

pub(crate) fn parse_local_state(bytes: &[u8]) -> Vec<ProfileInfo> {
    let Ok(value) = serde_json::from_slice::<Value>(bytes) else {
        return Vec::new();
    };
    let Some(cache) = value
        .pointer("/profile/info_cache")
        .and_then(Value::as_object)
    else {
        return Vec::new();
    };
    let mut out: Vec<ProfileInfo> = cache
        .iter()
        .filter(|(dir, _)| valid_profile_dir(dir))
        .map(|(dir, info)| ProfileInfo {
            dir: dir.clone(),
            name: info
                .get("name")
                .and_then(Value::as_str)
                .filter(|n| !n.trim().is_empty())
                .unwrap_or(dir)
                .to_string(),
        })
        .collect();
    out.sort_by(|a, b| a.dir.cmp(&b.dir));
    out
}

/// A profile directory is one path segment: it ends up on a command line and in a path.
pub(crate) fn valid_profile_dir(dir: &str) -> bool {
    !dir.is_empty()
        && dir.len() <= 64
        && dir != "."
        && dir != ".."
        && !dir.contains(['/', '\\', '\0'])
        && !dir.starts_with('-')
}

/// Where Chrome keeps its user data, per OS, under `home` (and `local_app_data` on Windows).
pub(crate) fn chrome_user_data_dir(home: &Path, local_app_data: Option<&Path>) -> Option<PathBuf> {
    if cfg!(target_os = "macos") {
        Some(home.join("Library/Application Support/Google/Chrome"))
    } else if cfg!(windows) {
        local_app_data.map(|d| d.join("Google/Chrome/User Data"))
    } else {
        Some(home.join(".config/google-chrome"))
    }
}
