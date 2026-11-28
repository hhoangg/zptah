//! The macOS side of the Ardia application: finding, quitting and hiding it through AppKit.
//! `osascript` would need the Apple Events permission, which this app (hardened runtime, no
//! apple-events entitlement) does not have, so the app registers this platform with the core.
use monocode_core::mono::browser::ardia::platform::{BUNDLE_ID, Platform, RealPlatform};
use objc2_app_kit::{NSRunningApplication, NSWorkspace};
use objc2_foundation::NSString;
use std::sync::Arc;

struct MacPlatform;

fn running() -> Option<objc2::rc::Retained<NSRunningApplication>> {
    let id = NSString::from_str(BUNDLE_ID);
    NSRunningApplication::runningApplicationsWithBundleIdentifier(&id).firstObject()
}

impl Platform for MacPlatform {
    fn can_launch(&self) -> bool {
        true
    }
    /// LaunchServices knows the bundle id wherever the app was copied to.
    fn installed(&self) -> bool {
        let id = NSString::from_str(BUNDLE_ID);
        NSWorkspace::sharedWorkspace()
            .URLForApplicationWithBundleIdentifier(&id)
            .is_some()
    }
    fn launch(&self) -> Result<(), String> {
        RealPlatform.launch()
    }
    /// A graceful quit request, like Cmd+Q in the app (Ardia may ask first when it is set to).
    fn quit(&self) -> Result<(), String> {
        match running() {
            Some(app) if app.terminate() => Ok(()),
            Some(_) => Err("Ardia did not accept the quit request".into()),
            None => Ok(()),
        }
    }
    fn is_hidden(&self) -> Option<bool> {
        running().map(|app| app.isHidden())
    }
}

/// Registers the macOS platform with the core. Call it before the browser is first used.
pub fn register() {
    monocode_core::mono::browser::ardia::controller::set_platform(Arc::new(MacPlatform));
}
