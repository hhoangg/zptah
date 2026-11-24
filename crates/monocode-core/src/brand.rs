//! zptah identity. Names here are product copy and bundle identifiers; file names,
//! env vars and protocol strings keep their `monocode` spelling.

/// The product name the user reads.
pub const PRODUCT: &str = "zptah";
/// The native app's own name.
pub const NATIVE_PRODUCT: &str = "zptah";
/// Bundle identifier of the app; it names the release data, log and WebKit directories.
pub const DESKTOP_APP_ID: &str = "app.zptah.desktop";
/// Bundle identifier of the native app; the updater matches packages on it.
pub const NATIVE_APP_ID: &str = "app.zptah.desktop";
/// Name of the isolated development data directory under the platform data dir.
pub const DEV_DATA_DIR: &str = "zptah Development";
/// File name of the database, in both the release and the development data directory.
/// The attachments directory (`<db>.threads`) and the sidecar databases derive from it.
pub const DATABASE_FILE: &str = "zptah.db";
/// Bundle identifier of the Tauri app this one replaces. Its data is never opened; the
/// credential guard still keeps agents out of it.
pub const MONOPTAH_DESKTOP_APP_ID: &str = "com.monoptah.desktop";
/// Bundle identifier of the Monoptah Native app, which runs side by side with this one.
pub const MONOPTAH_NATIVE_APP_ID: &str = "com.monoptah.native";
/// Data directory name of Monoptah Native's development build.
pub const MONOPTAH_DEV_DATA_DIR: &str = "Monoptah Development";

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn zptah_never_shares_an_identifier_with_monoptah() {
        assert_eq!(NATIVE_PRODUCT, "zptah");
        assert_eq!(DESKTOP_APP_ID, "app.zptah.desktop");
        assert_eq!(NATIVE_APP_ID, "app.zptah.desktop");
        assert_ne!(DESKTOP_APP_ID, MONOPTAH_DESKTOP_APP_ID);
        assert_ne!(NATIVE_APP_ID, MONOPTAH_NATIVE_APP_ID);
        assert_ne!(DEV_DATA_DIR, MONOPTAH_DEV_DATA_DIR);
        assert_eq!(DATABASE_FILE, "zptah.db");
    }
}
