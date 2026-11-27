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

    pub fn installed() -> bool {
        // LaunchServices knows the bundle id wherever the app was copied to.
        Command::new("mdfind")
            .arg(format!("kMDItemCFBundleIdentifier == '{BUNDLE_ID}'"))
            .output()
            .is_ok_and(|out| !String::from_utf8_lossy(&out.stdout).trim().is_empty())
    }
    pub fn launch() -> Result<(), String> {
        // `-g` background, `-j` hidden: measured not to steal focus or show a window.
        let status = Command::new("open")
            .args(["-g", "-j", "-a", "Ardia"])
            .status()
            .map_err(|e| e.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| "open could not start Ardia".to_owned())
    }
    pub fn quit() -> Result<(), String> {
        let script = format!("tell application id \"{BUNDLE_ID}\" to quit");
        let status = Command::new("osascript")
            .args(["-e", &script])
            .status()
            .map_err(|e| e.to_string())?;
        status
            .success()
            .then_some(())
            .ok_or_else(|| "Ardia did not accept the quit request".to_owned())
    }
    pub fn is_hidden() -> Option<bool> {
        let script = format!(
            "ObjC.import('AppKit'); const a = $.NSRunningApplication.runningApplicationsWithBundleIdentifier('{BUNDLE_ID}'); a.count > 0 ? String(a.objectAtIndex(0).hidden) : 'none'"
        );
        let out = Command::new("osascript")
            .args(["-l", "JavaScript", "-e", &script])
            .output()
            .ok()?;
        parse_hidden(&String::from_utf8_lossy(&out.stdout))
    }
    pub fn parse_hidden(text: &str) -> Option<bool> {
        match text.trim() {
            "true" => Some(true),
            "false" => Some(false),
            _ => None,
        }
    }
}

impl Platform for RealPlatform {
    fn can_launch(&self) -> bool {
        cfg!(target_os = "macos")
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
        #[cfg(target_os = "macos")]
        {
            mac::quit()
        }
        #[cfg(not(target_os = "macos"))]
        {
            Err("zptah can not quit Ardia on this system.".into())
        }
    }
    fn is_hidden(&self) -> Option<bool> {
        #[cfg(target_os = "macos")]
        {
            mac::is_hidden()
        }
        #[cfg(not(target_os = "macos"))]
        {
            None
        }
    }
}

#[cfg(all(test, target_os = "macos"))]
mod tests {
    #[test]
    fn hidden_flag_is_read_from_the_script_output() {
        assert_eq!(super::mac::parse_hidden("true\n"), Some(true));
        assert_eq!(super::mac::parse_hidden("false"), Some(false));
        assert_eq!(super::mac::parse_hidden("none"), None);
    }
}
