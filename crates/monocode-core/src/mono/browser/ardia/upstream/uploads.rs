//! Which files `browser_upload` may send (design spec section 6, "Uploads").
//!
//! Allowed: a file inside the Mono's project folder or its output folder, or a path the user gave
//! in the conversation (verbatim). Always refused, even inside an allowed folder: dotfiles and
//! dot folders (`.env`, `.git/`, ...), the places that hold credentials (`~/.ssh`, `~/.aws`,
//! `~/.gnupg`, `~/.config/gcloud`, `~/.kube`, keychains), the browsers' and chat apps' stores,
//! Ardia's own data folders and the app's own data folders. Every check is on the path after symlinks are resolved, so a link
//! into `~/.ssh` is the file it points to. The file must exist and be a regular file.
use crate::credential_guard::{
    all_app_data_dirs, INDEXED_HOME_PATHS, MORE_HOME_CREDENTIALS, SECRET_STORE_DIRS,
};
use std::path::{Component, Path, PathBuf};

/// The most files one upload may name.
pub const MAX_FILES: usize = 10;

/// Where an upload may come from.
#[derive(Clone, Debug, Default)]
pub struct Rules {
    /// The Mono's project folders and its output folder.
    pub allowed: Vec<PathBuf>,
    /// The output folder (inside the app's data folder, so the app-data rule skips it).
    pub output_dir: Option<PathBuf>,
    /// The user's home folder, where the credential places are.
    pub home: Option<PathBuf>,
    /// What the user wrote in this conversation: a path in a message is allowed verbatim.
    pub user_messages: Vec<String>,
}

/// One file that may be uploaded.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct UploadFile {
    /// The absolute path after symlinks: what is sent to `ardia` and shown on the card.
    pub path: PathBuf,
    pub size: u64,
    /// What the file was when it was checked (see [`UploadFile::recheck`]).
    stamp: Stamp,
}

/// The identity of a file at the time it was checked: size and modification time everywhere, and
/// the device and inode where there are such things.
#[derive(Clone, Debug, PartialEq, Eq)]
struct Stamp {
    size: u64,
    modified: Option<std::time::SystemTime>,
    #[cfg(unix)]
    dev: u64,
    #[cfg(unix)]
    ino: u64,
}

impl Stamp {
    fn of(meta: &std::fs::Metadata) -> Self {
        #[cfg(unix)]
        use std::os::unix::fs::MetadataExt;
        Self {
            size: meta.len(),
            modified: meta.modified().ok(),
            #[cfg(unix)]
            dev: meta.dev(),
            #[cfg(unix)]
            ino: meta.ino(),
        }
    }
}

/// A file with more than one hard link can be the same data as a file somewhere else (a link to
/// `~/.ssh/id_rsa` made inside the project folder): it is never uploaded.
fn has_hard_links(meta: &std::fs::Metadata) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        meta.nlink() > 1
    }
    #[cfg(not(unix))]
    {
        let _ = meta;
        false
    }
}

impl UploadFile {
    /// Right before the command: the path still resolves to itself (it is not a link now), and
    /// the file is the one that was checked (same device and inode, size and modification time).
    /// A file swapped while the user was asked is refused.
    pub fn recheck(&self) -> Result<(), String> {
        let shown = self.path.display();
        let again =
            std::fs::canonicalize(&self.path).map_err(|_| format!("the file `{shown}` is gone"))?;
        if again != self.path {
            return Err(format!(
                "the file `{shown}` now resolves to {}, so it is not the file that was checked",
                again.display()
            ));
        }
        let meta = std::fs::metadata(&again).map_err(|_| format!("the file `{shown}` is gone"))?;
        if !meta.is_file() || has_hard_links(&meta) || Stamp::of(&meta) != self.stamp {
            return Err(format!(
                "the file `{shown}` changed while the user was asked"
            ));
        }
        Ok(())
    }

    /// `"/abs/path (12.3 KB)"` for the card.
    pub fn describe(&self) -> String {
        format!("{} ({})", self.path.display(), human_size(self.size))
    }
}

pub fn human_size(bytes: u64) -> String {
    const UNITS: [&str; 4] = ["B", "KB", "MB", "GB"];
    let mut value = bytes as f64;
    let mut unit = 0;
    while value >= 1024.0 && unit + 1 < UNITS.len() {
        value /= 1024.0;
        unit += 1;
    }
    if unit == 0 {
        format!("{bytes} B")
    } else {
        format!("{value:.1} {}", UNITS[unit])
    }
}

fn canonical(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// Whether `text` holds `path` as a word of its own (not a part of a longer path).
pub fn mentioned_verbatim(path: &str, text: &str) -> bool {
    if path.is_empty() {
        return false;
    }
    let path_char =
        |c: char| c.is_alphanumeric() || matches!(c, '/' | '\\' | '_' | '-' | '.' | '~');
    let mut from = 0;
    while let Some(found) = text[from..].find(path) {
        let start = from + found;
        let end = start + path.len();
        let before_ok = text[..start]
            .chars()
            .next_back()
            .is_none_or(|c| !path_char(c));
        let after = &text[end..];
        let after_ok = match after.chars().next() {
            None => true,
            // A sentence's full stop or comma after the path is not part of it.
            Some('.' | ',' | ';' | ':' | '!' | '?') => after[1..]
                .chars()
                .next()
                .is_none_or(|c| !path_char(c) || c.is_whitespace()),
            Some(c) => !path_char(c),
        };
        if before_ok && after_ok {
            return true;
        }
        from = end;
    }
    false
}

/// A hidden component below `root` (or anywhere in the path when there is no root).
fn hidden_below(path: &Path, root: Option<&Path>) -> bool {
    let tail = match root.and_then(|r| path.strip_prefix(r).ok()) {
        Some(tail) => tail,
        None => path,
    };
    tail.components().any(|c| match c {
        Component::Normal(name) => name.to_string_lossy().starts_with('.'),
        _ => false,
    })
}

fn is_keychain(path: &Path) -> bool {
    path.extension().is_some_and(|e| {
        let e = e.to_string_lossy().to_lowercase();
        e == "keychain-db" || e == "keychain" || e == "kdbx"
    })
}

/// The data folders of the Ardia browser (cookies, logins, history): never uploaded. The macOS,
/// Linux and Windows places, under `home` and under the environment's `APPDATA` and
/// `LOCALAPPDATA` when they are set.
pub fn ardia_data_dirs(home: &Path) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = [
        "Library/Application Support/Ardia",
        "Library/Application Support/com.ardia.browser",
        ".config/Ardia",
        ".config/ardia",
        ".local/share/Ardia",
        ".local/share/ardia",
        "AppData/Roaming/Ardia",
        "AppData/Local/Ardia",
    ]
    .iter()
    .map(|tail| home.join(tail))
    .collect();
    for var in ["APPDATA", "LOCALAPPDATA"] {
        if let Some(base) = std::env::var_os(var) {
            dirs.push(PathBuf::from(base).join("Ardia"));
        }
    }
    dirs
}

/// Credential files and the stores of other programs, and the app's own data (the same lists the
/// remote file read refuses), plus keychains. `canonical` is a resolved path.
pub fn credential_place(canonical: &Path, home: &Path, output_dir: Option<&Path>) -> bool {
    if is_keychain(canonical) {
        return true;
    }
    let home = canonical_dir(home);
    let outputs = output_dir.map(canonical_dir);
    let in_output = outputs.as_ref().is_some_and(|o| canonical.starts_with(o));
    let listed = INDEXED_HOME_PATHS
        .iter()
        .chain(MORE_HOME_CREDENTIALS.iter())
        .chain(SECRET_STORE_DIRS.iter())
        .map(|tail| home.join(tail))
        .chain(std::iter::once(home.join("Library/Keychains")))
        .chain(std::iter::once(home.join(".local/share/keyrings")))
        .chain(ardia_data_dirs(&home))
        .any(|dir| canonical.starts_with(&dir) || canonical.starts_with(canonical_dir(&dir)));
    if listed {
        return true;
    }
    // The Mono's output folder lives inside the app's data folder and is allowed on purpose.
    !in_output
        && all_app_data_dirs(&home)
            .into_iter()
            .any(|dir| canonical.starts_with(&dir) || canonical.starts_with(canonical_dir(&dir)))
}

fn canonical_dir(path: &Path) -> PathBuf {
    canonical(path)
}

/// Checks the paths of one `browser_upload` call. `Err` is the sentence for the Mono.
pub fn check(paths: &[String], rules: &Rules) -> Result<Vec<UploadFile>, String> {
    if paths.is_empty() || paths.len() > MAX_FILES {
        return Err(format!("`paths` needs one to {MAX_FILES} files."));
    }
    let Some(home) = rules.home.as_deref() else {
        return Err("Uploads are not available: the home folder is unknown, so credential files cannot be ruled out.".into());
    };
    let allowed: Vec<PathBuf> = rules.allowed.iter().map(|p| canonical(p)).collect();
    let mut files = Vec::new();
    for raw in paths {
        if raw.contains('\0') || raw.trim().is_empty() {
            return Err("A path in `paths` is empty or has a NUL character.".into());
        }
        let given = Path::new(raw);
        if !given.is_absolute() {
            return Err(format!(
                "`paths` must be absolute file paths (got `{raw}`)."
            ));
        }
        let resolved = std::fs::canonicalize(given)
            .map_err(|_| format!("The file `{raw}` does not exist or cannot be read."))?;
        let meta = std::fs::metadata(&resolved)
            .map_err(|_| format!("The file `{raw}` cannot be read."))?;
        if !meta.is_file() {
            return Err(format!(
                "`{raw}` is not a regular file (a folder is never uploaded)."
            ));
        }
        if has_hard_links(&meta) {
            return Err(format!(
                "`{raw}` has more than one hard link, so it could be the same file as one that is never uploaded."
            ));
        }
        if credential_place(&resolved, home, rules.output_dir.as_deref()) {
            return Err(format!(
                "`{raw}` holds credentials or the app's own data, so it is never uploaded."
            ));
        }
        let root = allowed.iter().find(|a| resolved.starts_with(a));
        let by_user = rules
            .user_messages
            .iter()
            .any(|m| mentioned_verbatim(raw, m));
        match root {
            Some(root) => {
                if hidden_below(&resolved, Some(root)) {
                    return Err(format!(
                        "`{raw}` is a dotfile or inside a dot folder, so it is never uploaded."
                    ));
                }
            }
            None if by_user => {
                let home = canonical(home);
                let below_home = resolved.starts_with(&home).then_some(home.as_path());
                if hidden_below(&resolved, below_home) {
                    return Err(format!(
                        "`{raw}` is a dotfile or inside a dot folder, so it is never uploaded."
                    ));
                }
            }
            None => {
                return Err(format!(
                    "`{raw}` is outside this Mono's project folder and its output folder, and the user did not name that path. Ask the user for it."
                ))
            }
        }
        files.push(UploadFile {
            path: resolved,
            size: meta.len(),
            stamp: Stamp::of(&meta),
        });
    }
    Ok(files)
}

/// The user's home folder.
pub fn home_dir() -> Option<PathBuf> {
    std::env::var_os("HOME")
        .or_else(|| std::env::var_os("USERPROFILE"))
        .map(PathBuf::from)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_path_counts_as_given_only_as_a_word_of_its_own() {
        let text = "please upload /Users/user/docs/cv.pdf. thanks";
        assert!(mentioned_verbatim("/Users/user/docs/cv.pdf", text));
        assert!(mentioned_verbatim(
            "/Users/user/docs/cv.pdf",
            "(/Users/user/docs/cv.pdf)"
        ));
        assert!(!mentioned_verbatim("/Users/user/docs/cv", text));
        assert!(!mentioned_verbatim(
            "/Users/user/docs/cv.pdf",
            "see /Users/user/docs/cv.pdf2"
        ));
        assert!(!mentioned_verbatim(
            "/Users/user/docs",
            "/Users/user/docs/cv.pdf"
        ));
        assert!(!mentioned_verbatim("", "anything"));
    }

    #[test]
    fn sizes_are_readable() {
        assert_eq!(human_size(12), "12 B");
        assert_eq!(human_size(2048), "2.0 KB");
        assert_eq!(human_size(5 * 1024 * 1024), "5.0 MB");
    }
}
