//! Names the app uses for the zptah Host on a remote machine, and the ones it must never touch.
//!
//! The zptah Host (`zptah-host`) runs side by side with Monoptah's `monocode-host` on the same
//! machine and shares nothing with it: not the install or data directory, not the launchd
//! label, systemd unit or Windows task, not the lock or pid files, not the port. The zptah app
//! only ever runs, probes, upgrades or removes the names below. It never stops, upgrades,
//! uninstalls or deletes `monocode-host` or `~/.monocode-host`: that install may belong to the
//! Monoptah app.
//!
//! There is deliberately no migration of state from `~/.monocode-host`: no zptah Host has been
//! installed on any remote yet, and a fresh install pairs a fresh device credential.

/// The Host CLI the app installs and runs (`~/.zptah-host/bin/zptah-host`).
pub const HOST_CLI: &str = "zptah-host";
/// Install and data directory under the user's home directory.
pub const HOST_DIR: &str = ".zptah-host";
/// launchd label of the macOS service (the Host itself owns it; kept here for the tests).
pub const HOST_LAUNCHD_LABEL: &str = "app.zptah.host";
/// Port the zptah Host listens on unless told otherwise.
pub const DEFAULT_HOST_PORT: u16 = 3784;
/// Default port of Monoptah's `monocode-host`. The zptah Host never binds it.
pub const MONOCODE_HOST_PORT: u16 = 3774;

/// The port to give the bootstrap for a machine whose saved port is `saved`. A machine that was
/// connected to a `monocode-host` has `3774` saved; the zptah Host must not take it over, so it
/// gets its own default. Any other port (the one a zptah Host reported) is kept.
pub fn install_port(saved: u16) -> u16 {
    if saved == MONOCODE_HOST_PORT {
        DEFAULT_HOST_PORT
    } else {
        saved
    }
}

/// The package name a pin must carry before the app may install or upgrade a Host.
pub const REQUIRED_PACKAGE_NAME: &str = HOST_CLI;

/// The pin rule: only a release of the zptah Host (`zptah-host` package, `+zptah` flavor) may be
/// installed. An older pin (1.2.x, `monocode-host` package) installs a binary that acts on
/// the names of Monoptah's Host (its launchd label, systemd unit, Windows task
/// and `~/.monocode-host`), so the app refuses before any remote command runs. The refusal
/// disappears by itself once `host_release.rs` is re-pinned to a zptah-host release.
pub fn check_pin(package: &str, version: &str) -> Result<(), String> {
    if package == REQUIRED_PACKAGE_NAME && version.ends_with("+zptah") {
        Ok(())
    } else {
        Err("The zptah Host is not available in this build yet. Update zptah when a build with the Host release is published.".into())
    }
}

/// [`check_pin`] for the current pin. Call it before generating or sending any Host bootstrap
/// or upgrade script (SSH, codespaces, cloud sessions, sandboxes). Unit tests of the flows run
/// against fake machines with the interim pin, so they skip it; `check_pin` itself is tested.
pub fn ensure_host_available() -> Result<(), String> {
    if cfg!(test) {
        return Ok(());
    }
    check_pin(
        crate::remote::host_release::HOST_PACKAGE_NAME,
        crate::remote::host_release::HOST_VERSION,
    )
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_current_pin_is_refused_until_it_is_a_zptah_host_release() {
        use crate::remote::host_release::{HOST_PACKAGE_NAME, HOST_VERSION};
        let usable = HOST_PACKAGE_NAME == "zptah-host" && HOST_VERSION.ends_with("+zptah");
        assert_eq!(check_pin(HOST_PACKAGE_NAME, HOST_VERSION).is_ok(), usable);
        // The interim pin is refused with a clear message.
        let error = check_pin("monocode-host", "1.2.1+monoptah").unwrap_err();
        assert!(error.contains("not available in this build yet"), "{error}");
        assert!(check_pin("zptah-host", "1.2.1+monoptah").is_err());
        assert!(check_pin("monocode-host", "1.4.0+zptah").is_err());
        assert!(check_pin("zptah-host", "1.4.0+zptah").is_ok());
    }

    /// Every place that sends a Host bootstrap or upgrade must ask first.
    #[test]
    fn every_bootstrap_sender_checks_the_pin_first() {
        for (source, calls) in [
            (include_str!("../remote.rs"), 2),
            (include_str!("cloud_upgrade.rs"), 1),
            (include_str!("../cloud_sessions/docker_start.rs"), 1),
        ] {
            assert_eq!(source.matches("ensure_host_available()?").count(), calls);
        }
    }

    #[test]
    fn names_are_distinct_from_monocode_host() {
        assert_eq!(HOST_CLI, "zptah-host");
        assert_eq!(HOST_DIR, ".zptah-host");
        assert_eq!(HOST_LAUNCHD_LABEL, "app.zptah.host");
        assert_ne!(DEFAULT_HOST_PORT, MONOCODE_HOST_PORT);
        for name in [HOST_CLI, HOST_DIR, HOST_LAUNCHD_LABEL] {
            assert!(!name.contains("monocode"), "{name}");
        }
    }

    #[test]
    fn the_monocode_host_port_is_never_reused() {
        assert_eq!(install_port(3774), DEFAULT_HOST_PORT);
        assert_eq!(install_port(3784), 3784);
        assert_eq!(install_port(3790), 3790);
    }

    #[test]
    fn an_upgrade_of_a_machine_saved_with_the_monocode_host_port_uses_the_zptah_port() {
        use crate::remote::ssh::{upgrade_script, HostPlatform};
        assert!(upgrade_script(HostPlatform::Unix, 3774)
            .starts_with("ZPTAH_HOST_FORCE_UPGRADE=1\nZPTAH_HOST_PORT=3784\n"));
        assert!(
            upgrade_script(HostPlatform::Windows, 3774).contains("$env:ZPTAH_HOST_PORT = '3784'")
        );
        assert!(upgrade_script(HostPlatform::Unix, 3790).contains("ZPTAH_HOST_PORT=3790\n"));
    }

    /// Everything the app sends to a machine (all four bootstraps, upgrade, pairing, the
    /// sandbox relay) must name the zptah paths only. The package name inside the pinned
    /// release is the one allowed exception, until the pin moves to a `zptah-host` release.
    #[test]
    fn scripts_never_name_monocode_host_paths() {
        use crate::remote::ssh::{
            bootstrap_script, codespace_bootstrap_script, codespace_forget_token_marker,
            codespace_stop_host_prestep, pairing_script, sandbox_bootstrap_script, upgrade_script,
            HostPlatform,
        };
        let package = crate::remote::host_release::HOST_PACKAGE_NAME;
        let mut scripts = vec![
            codespace_bootstrap_script(),
            sandbox_bootstrap_script(None),
            codespace_stop_host_prestep().to_owned(),
            codespace_forget_token_marker().to_owned(),
            include_str!("../cloud_sessions/backend/docker_relay.sh").to_owned(),
        ];
        for platform in [HostPlatform::Unix, HostPlatform::Windows] {
            scripts.push(bootstrap_script(platform));
            scripts.push(upgrade_script(platform, 3774));
            scripts.push(pairing_script(platform, "name"));
        }
        for script in scripts {
            // Only while the pin is the old `monocode-host` package: its name appears as the
            // asset prefix and the executable inside the package, never as a path. After the
            // re-pin this replace is a no-op and the exception can go.
            let mut script = script;
            for allowed in [
                "FILE=\"{}-$OS-$ARCH.tar.gz\"",
                "\"$1/{}\" --version",
                "\"$DEST/{}\" --version",
                "exec \"$RUNTIME/{}\"",
                "\"{}-$target.zip\"",
            ] {
                script = script.replace(&allowed.replace("{}", package), "ALLOWED");
            }
            assert!(!script.contains(".monocode-host"), "{script}");
            assert!(!script.contains("com.monocode.host"), "{script}");
            assert!(!script.contains("monocode-host"), "{script}");
            assert!(!script.contains("MONOCODE_HOST_"), "{script}");
            assert!(!script.contains("3774"), "{script}");
        }
    }

    #[test]
    fn the_install_script_installs_into_the_zptah_directory() {
        let unix = crate::remote::ssh::bootstrap_script(crate::remote::ssh::HostPlatform::Unix);
        assert!(unix.contains(&format!("BASE=\"$HOME/{HOST_DIR}\"")));
        assert!(unix.contains(&format!("ENTRY=\"$BASE/bin/{HOST_CLI}\"")));
        assert!(unix.contains("HOST_PORT=${ZPTAH_HOST_PORT:-3784}"));
        let windows =
            crate::remote::ssh::bootstrap_script(crate::remote::ssh::HostPlatform::Windows);
        assert!(windows.contains(&format!("'{HOST_DIR}'")));
        assert!(windows.contains(&format!("'{HOST_CLI}.cmd'")));
    }
}
