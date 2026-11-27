//! What zptah does to the Ardia application itself: find it, start it, quit it, see whether it is
//! hidden. Behind a trait so tests on every platform fake it and never look up or start an app.
//!
//! Only macOS can launch and quit Ardia in v1 (design spec section 12b, user decision): on Linux
//! and Windows a closed Ardia fails the call with "Open Ardia".

/// The macOS bundle id of Ardia.
pub const BUNDLE_ID: &str = "com.ardia.browser";

pub trait Platform: Send + Sync {
    /// Whether zptah may start and quit Ardia here. False on Linux and Windows.
    fn can_launch(&self) -> bool;
    /// Whether the Ardia application is installed (macOS: found by LaunchServices).
    fn installed(&self) -> bool;
    /// Starts Ardia in the background without taking focus. Returns once it was asked to start.
    fn launch(&self) -> Result<(), String>;
    /// Asks Ardia to quit, gracefully.
    fn quit(&self) -> Result<(), String>;
    /// Whether the running Ardia is hidden; `None` when that can not be told.
    fn is_hidden(&self) -> Option<bool>;
}

/// The machine's real platform.
pub struct RealPlatform;

#[cfg(target_os = "macos")]
mod mac {
    use super::BUNDLE_ID;
    use std::process::Command;

    /// The Ardia application is in one of the two usual folders. The app registers a platform
    /// that asks LaunchServices instead (`NSWorkspace`), see `monocode-gpui/src/ardia_platform.rs`.
    pub fn installed() -> bool {
        let home = crate::scm::shell::dirs_home().map(std::path::PathBuf::from);
        let mut places = vec![std::path::PathBuf::from("/Applications/Ardia.app")];
        places.extend(home.map(|h| h.join("Applications/Ardia.app")));
        places.iter().any(|p| p.is_dir())
    }
    pub fn launch() -> Result<(), String> {
        // `-g` background, `-j` hidden: measured not to steal focus or show a window. By bundle
        // id, so a renamed or moved app still starts.
        let status = Command::new("open")
            .args(["-g", "-j", "-b", BUNDLE_ID])
            .status()
            .map_err(|e| e.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| "open could not start Ardia".to_owned())
    }
}

impl Platform for RealPlatform {
    /// Quitting needs AppKit, which the app's platform provides (`set_platform`). This one is
    /// only the fallback, and a Ardia that can be started but not quit would stay open for ever,
    /// so it never launches.
    fn can_launch(&self) -> bool {
        false
    }
    fn installed(&self) -> bool {
        #[cfg(target_os = "macos")]
        {
            mac::installed()
        }
        // Elsewhere the `ardia` command is what Ardia installs; the PATH check covers it.
        #[cfg(not(target_os = "macos"))]
        {
            true
        }
    }
    fn launch(&self) -> Result<(), String> {
        #[cfg(target_os = "macos")]
        {
            mac::launch()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("Open Ardia yourself on this system.".into())
        }
    }
    fn quit(&self) -> Result<(), String> {
        // Quitting needs AppKit (no Apple Events permission): the app's platform does it.
        Err("zptah can not quit Ardia here.".into())
    }
    fn is_hidden(&self) -> Option<bool> {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_fallback_platform_never_launches_because_it_can_not_quit() {
        assert!(!RealPlatform.can_launch());
        assert!(RealPlatform.quit().is_err());
        assert_eq!(RealPlatform.is_hidden(), None);
    }
}
