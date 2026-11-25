use super::parse::*;
use std::path::{Path, PathBuf};

#[test]
fn proc_stat_reads_ppid_and_start_time_after_a_tricky_comm() {
    let stat = "4242 (we ird) name) S 77 4242 4242 0 -1 4194560 100 0 0 0 1 2 0 0 20 0 1 0 987654 1000 10 18446744073709551615";
    assert_eq!(parse_proc_stat(stat), Some((77, 987654)));
    assert_eq!(parse_proc_stat("garbage"), None);
    assert_eq!(parse_proc_stat("1 (a) S 2 3"), None);
}

#[test]
fn user_data_dir_takes_the_last_flag_in_either_form() {
    assert_eq!(
        user_data_dir_from_args(&["chrome", "--user-data-dir=/a", "--user-data-dir", "/b"]),
        Some(PathBuf::from("/b"))
    );
    assert_eq!(
        user_data_dir_from_args(&["chrome", "--user-data-dir=/a"]),
        Some(PathBuf::from("/a"))
    );
    assert_eq!(
        user_data_dir_from_args(&["chrome", "--user-data-dir="]),
        None
    );
    assert_eq!(
        user_data_dir_from_args(&["chrome", "--profile-directory=x"]),
        None
    );
}

#[test]
fn user_data_dir_from_cmdline_splits_on_nul() {
    let raw = b"/opt/google/chrome/chrome\0--user-data-dir=/home/u/p q\0--flag\0";
    assert_eq!(
        user_data_dir_from_cmdline(raw),
        Some(PathBuf::from("/home/u/p q"))
    );
    assert_eq!(user_data_dir_from_cmdline(b""), None);
}

#[test]
fn user_data_dir_from_procargs2_skips_exe_padding_and_stops_at_argc() {
    let mut raw = 3i32.to_ne_bytes().to_vec();
    raw.extend_from_slice(b"/Applications/Chrome\0\0\0\0");
    raw.extend_from_slice(b"chrome\0--user-data-dir=/p\0--x\0");
    // The environment follows the arguments and must not be read as arguments.
    raw.extend_from_slice(b"HOME=/h\0--user-data-dir=/env\0");
    assert_eq!(
        user_data_dir_from_procargs2(&raw),
        Some(PathBuf::from("/p"))
    );
    assert_eq!(user_data_dir_from_procargs2(b"\x01"), None);
}

#[test]
fn only_known_chrome_paths_are_browsers() {
    assert!(is_chrome_linux_exe(Path::new("/opt/google/chrome/chrome")));
    assert!(!is_chrome_linux_exe(Path::new("/usr/bin/zptah")));
    assert!(!is_chrome_linux_exe(Path::new(
        "/tmp/opt/google/chrome/chrome"
    )));
    assert!(is_chrome_windows_exe(
        r"C:\Program Files\Google\Chrome\Application\chrome.exe"
    ));
    assert!(is_chrome_windows_exe(
        r"c:\users\a\appdata\local\google\chrome sxs\application\chrome.exe"
    ));
    assert!(!is_chrome_windows_exe(r"C:\evil\chrome.exe"));
    assert!(!is_chrome_windows_exe(
        r"C:\Program Files\Mozilla Firefox\firefox.exe"
    ));
    assert!(is_windows_shim(r"C:\Windows\System32\cmd.exe"));
    assert!(!is_windows_shim(r"C:\Temp\cmd.exe"));
}

#[test]
fn pipe_name_is_stable_per_user_and_separates_dev() {
    let a = pipe_name_for_sid("S-1-5-21-1", false);
    assert_eq!(a, pipe_name_for_sid("S-1-5-21-1", false));
    assert_ne!(a, pipe_name_for_sid("S-1-5-21-2", false));
    assert_eq!(pipe_name_for_sid("S-1-5-21-1", true), format!("{a}-dev"));
    assert!(a.starts_with(r"\\.\pipe\zptah-browser-"));
    assert!(!a.contains("S-1-5"));
}

#[test]
fn chrome_requirement_lists_the_google_team_and_chrome_identifiers() {
    assert!(CHROME_REQUIREMENT.contains("EQHXZ8M8AV"));
    assert!(CHROME_REQUIREMENT.contains("identifier \"com.google.Chrome\""));
}

#[cfg(target_os = "macos")]
#[test]
fn chrome_requirement_matches_the_installed_chrome() {
    let app = Path::new("/Applications/Google Chrome.app");
    if !app.exists() {
        return;
    }
    let out = std::process::Command::new("/usr/bin/codesign")
        .arg("--verify")
        .arg(format!("-R={CHROME_REQUIREMENT}"))
        .arg(app)
        .output()
        .expect("run codesign");
    assert!(
        out.status.success(),
        "codesign: {}",
        String::from_utf8_lossy(&out.stderr)
    );
}

#[cfg(unix)]
mod unix_tests {
    use super::super::dir::*;
    use super::super::lock::*;
    use super::super::{
        inspect_own_parent, verify_app_peer, verify_host_peer, Flavor, PeerError, PeerPolicy,
    };
    use std::io::{BufRead, BufReader, Read, Write};
    use std::os::unix::fs::{symlink, PermissionsExt};
    use std::os::unix::net::{UnixListener, UnixStream};
    use std::path::{Path, PathBuf};
    use std::process::{Child, ChildStdin, Command, Stdio};

    const ROLE: &str = "ZPTAH_PEER_TEST_ROLE";
    const DIR: &str = "ZPTAH_PEER_TEST_DIR";

    fn private_dir() -> tempfile::TempDir {
        let t = tempfile::tempdir().expect("temp dir");
        std::fs::set_permissions(t.path(), std::fs::Permissions::from_mode(0o700)).expect("chmod");
        t
    }

    // ---- child processes (this test binary re-executed) ----

    /// Runs only when a peer test re-executes this binary with a role. It holds the lock and or the
    /// socket as asked, prints `ready`, then waits until the parent closes its stdin.
    #[test]
    #[ignore = "helper process re-executed by the peer tests, does nothing on its own"]
    fn child_helper() {
        let Ok(role) = std::env::var(ROLE) else {
            return;
        };
        let dir = PathBuf::from(std::env::var(DIR).expect("dir"));
        let sock = socket_path(&dir, Flavor::Dev).expect("socket path");
        let mut keep: Vec<Box<dyn std::any::Any>> = Vec::new();
        match role.as_str() {
            "lock" => {
                keep.push(Box::new(
                    InstanceLock::acquire(&dir, Flavor::Dev).expect("lock"),
                ));
            }
            "connect" => {
                keep.push(Box::new(UnixStream::connect(&sock).expect("connect")));
            }
            "serve" => {
                keep.push(Box::new(UnixListener::bind(&sock).expect("bind")));
            }
            "serve_lock" => {
                keep.push(Box::new(
                    InstanceLock::acquire(&dir, Flavor::Dev).expect("lock"),
                ));
                keep.push(Box::new(UnixListener::bind(&sock).expect("bind")));
            }
            other => panic!("unknown role {other}"),
        }
        println!("ready");
        std::io::stdout().flush().expect("flush");
        let mut sink = Vec::new();
        let _ = std::io::stdin().read_to_end(&mut sink);
        drop(keep);
    }

    struct Helper {
        child: Child,
        stdin: Option<ChildStdin>,
    }

    impl Helper {
        fn pid(&self) -> u32 {
            self.child.id()
        }

        fn stop(&mut self) {
            self.stdin.take();
            let _ = self.child.wait();
        }
    }

    impl Drop for Helper {
        fn drop(&mut self) {
            self.stop();
        }
    }

    fn helper_test_name() -> String {
        let module = module_path!().split_once("::").expect("crate prefix").1;
        format!("{module}::child_helper")
    }

    fn spawn_helper(exe: &Path, role: &str, dir: &Path) -> Helper {
        let mut child = Command::new(exe)
            .args(["--ignored", "--exact", &helper_test_name(), "--nocapture"])
            .env(ROLE, role)
            .env(DIR, dir)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .spawn()
            .expect("spawn helper");
        let stdin = child.stdin.take();
        let mut out = BufReader::new(child.stdout.take().expect("stdout"));
        let mut line = String::new();
        loop {
            line.clear();
            let n = out.read_line(&mut line).expect("read helper output");
            assert!(n > 0, "helper {role} exited before it was ready");
            if line.trim() == "ready" {
                break;
            }
        }
        Helper { child, stdin }
    }

    fn own_exe() -> PathBuf {
        std::env::current_exe().expect("current exe")
    }

    /// A dev policy that accepts this test process as the browser, since the helper's parent is
    /// the test process, not Chrome.
    fn test_browser_policy() -> PeerPolicy {
        PeerPolicy::native_dev().with_extra_browser(own_exe())
    }

    // ---- directory ----

    #[test]
    fn ensure_dir_creates_a_private_directory_and_reuses_it() {
        let parent = private_dir();
        let dir = parent.path().join("zptah");
        ensure_dir(&dir).expect("create");
        let mode = std::fs::metadata(&dir).expect("meta").permissions().mode() & 0o7777;
        assert_eq!(mode, 0o700);
        ensure_dir(&dir).expect("reuse");
        check_dir(&dir).expect("check");
    }

    #[test]
    fn check_dir_refuses_wrong_mode_symlink_file_and_other_owner() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("create");

        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o755)).expect("chmod");
        assert!(matches!(
            check_dir(&dir),
            Err(PeerError::WrongMode { mode: 0o755, .. })
        ));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o770)).expect("chmod");
        assert!(matches!(check_dir(&dir), Err(PeerError::WrongMode { .. })));
        std::fs::set_permissions(&dir, std::fs::Permissions::from_mode(0o700)).expect("chmod");

        let link = parent.path().join("link");
        symlink(&dir, &link).expect("symlink");
        assert!(matches!(check_dir(&link), Err(PeerError::Symlink(_))));
        assert!(matches!(ensure_dir(&link), Err(PeerError::Symlink(_))));

        let file = parent.path().join("file");
        std::fs::write(&file, b"x").expect("write");
        assert!(matches!(check_dir(&file), Err(PeerError::NotDirectory(_))));

        let other = current_uid().wrapping_add(1);
        assert!(matches!(
            check_dir_as(&dir, other),
            Err(PeerError::WrongOwner { .. })
        ));
        assert!(matches!(
            check_dir(&parent.path().join("missing")),
            Err(PeerError::Io(_))
        ));
    }

    #[cfg(not(target_os = "macos"))]
    #[test]
    fn linux_socket_dir_prefers_an_absolute_xdg_runtime_dir() {
        assert_eq!(
            linux_socket_dir(Some(PathBuf::from("/run/user/1000")), 1000),
            PathBuf::from("/run/user/1000/zptah")
        );
        assert_eq!(
            linux_socket_dir(Some(PathBuf::from("relative")), 1000),
            PathBuf::from("/tmp/zptah-1000")
        );
        assert_eq!(linux_socket_dir(None, 7), PathBuf::from("/tmp/zptah-7"));
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_socket_dir_ignores_tmpdir_and_fits_sun_path() {
        let dir = socket_dir().expect("dir");
        assert!(dir.ends_with(format!("zptah-{}", current_uid())));
        assert!(dir.is_absolute());
        socket_path(&dir, Flavor::Release).expect("fits sun_path");
    }

    #[test]
    fn socket_path_refuses_a_path_that_cannot_fit_sun_path() {
        let long = PathBuf::from(format!("/tmp/{}", "x".repeat(120)));
        assert!(matches!(
            socket_path(&long, Flavor::Release),
            Err(PeerError::PathTooLong(_))
        ));
        let ok = socket_path(Path::new("/tmp/zptah-1"), Flavor::Dev).expect("short");
        assert!(ok.ends_with("browser-dev.sock"));
    }

    #[test]
    fn remove_stale_socket_removes_only_sockets() {
        let parent = private_dir();
        let sock = parent.path().join("s.sock");
        remove_stale_socket(&sock).expect("missing is fine");
        drop(UnixListener::bind(&sock).expect("bind"));
        assert!(sock.exists());
        remove_stale_socket(&sock).expect("remove");
        assert!(!sock.exists());
        std::fs::write(&sock, b"x").expect("write");
        assert!(remove_stale_socket(&sock).is_err());
        std::fs::remove_file(&sock).expect("rm");
        symlink("/etc/hosts", &sock).expect("symlink");
        assert!(matches!(
            remove_stale_socket(&sock),
            Err(PeerError::Symlink(_))
        ));
    }

    // ---- lock ----

    #[test]
    fn second_process_cannot_take_the_lock_and_the_holder_pid_is_reported() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        assert_eq!(lock_holder_pid(&dir, Flavor::Dev).ok().flatten(), None);

        let mut holder = spawn_helper(&own_exe(), "lock", &dir);
        match InstanceLock::acquire(&dir, Flavor::Dev) {
            Err(PeerError::LockHeld(pid)) => assert_eq!(pid, Some(holder.pid())),
            other => panic!("expected LockHeld, got {other:?}"),
        }
        assert_eq!(
            lock_holder_pid(&dir, Flavor::Dev).expect("query"),
            Some(holder.pid())
        );
        // The release flavor has its own lock file.
        InstanceLock::acquire(&dir, Flavor::Release).expect("other flavor is independent");

        holder.stop();
        let lock = InstanceLock::acquire(&dir, Flavor::Dev).expect("free after holder exits");
        assert!(lock.path().ends_with("browser-dev.lock"));
    }

    #[test]
    fn lock_file_that_is_a_symlink_is_refused() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let target = parent.path().join("elsewhere");
        std::fs::write(&target, b"x").expect("write");
        symlink(&target, dir.join("browser-dev.lock")).expect("symlink");
        assert!(InstanceLock::acquire(&dir, Flavor::Dev).is_err());
        assert!(lock_holder_pid(&dir, Flavor::Dev).is_err());
    }

    // ---- peer verification ----

    fn accept_from_helper(dir: &Path, role_exe: &Path) -> (Helper, UnixStream) {
        let sock = socket_path(dir, Flavor::Dev).expect("path");
        let listener = UnixListener::bind(&sock).expect("bind");
        let helper = spawn_helper(role_exe, "connect", dir);
        let (stream, _) = listener.accept().expect("accept");
        (helper, stream)
    }

    #[test]
    fn host_peer_that_is_our_own_executable_under_a_browser_is_accepted() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let (helper, stream) = accept_from_helper(&dir, &own_exe());
        let report = verify_host_peer(&stream, &test_browser_policy()).expect("accepted");
        assert_eq!(report.pid, helper.pid());
        assert_eq!(report.parent_pid, Some(std::process::id()));
        assert_eq!(report.parent_exe.as_deref(), Some(own_exe().as_path()));
    }

    #[test]
    fn host_peer_whose_parent_is_not_a_browser_is_refused() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let (_helper, stream) = accept_from_helper(&dir, &own_exe());
        // The release policy knows only Google Chrome, and the parent here is the test binary.
        let err = verify_host_peer(&stream, &PeerPolicy::release()).expect_err("refused");
        assert!(matches!(err, PeerError::ParentNotBrowser(_)), "{err:?}");
    }

    #[test]
    fn host_peer_with_a_different_executable_is_refused() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let copy = parent.path().join("other-exe");
        std::fs::copy(own_exe(), &copy).expect("copy exe");
        let (_helper, stream) = accept_from_helper(&dir, &copy);
        let err = verify_host_peer(&stream, &test_browser_policy()).expect_err("refused");
        assert!(matches!(err, PeerError::ExeMismatch(_)), "{err:?}");
    }

    #[test]
    fn app_peer_is_accepted_only_when_it_serves_and_holds_the_lock() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let sock = socket_path(&dir, Flavor::Dev).expect("path");

        let server = spawn_helper(&own_exe(), "serve_lock", &dir);
        let stream = UnixStream::connect(&sock).expect("connect");
        let pid =
            verify_app_peer(&stream, &dir, Flavor::Dev, &PeerPolicy::release()).expect("accepted");
        assert_eq!(pid, server.pid());
        drop(stream);
        drop(server);
        std::fs::remove_file(&sock).expect("rm sock");

        // A second copy that serves the socket without holding the lock is refused.
        let mut holder = spawn_helper(&own_exe(), "lock", &dir);
        let rogue = spawn_helper(&own_exe(), "serve", &dir);
        let stream = UnixStream::connect(&sock).expect("connect");
        let err = verify_app_peer(&stream, &dir, Flavor::Dev, &PeerPolicy::release())
            .expect_err("refused");
        match err {
            PeerError::LockHolderMismatch { holder: h, peer } => {
                assert_eq!(h, Some(holder.pid()));
                assert_eq!(peer, rogue.pid());
            }
            other => panic!("expected LockHolderMismatch, got {other:?}"),
        }
        holder.stop();
    }

    #[test]
    fn app_peer_with_a_different_executable_is_refused() {
        let parent = private_dir();
        let dir = parent.path().join("d");
        ensure_dir(&dir).expect("dir");
        let copy = parent.path().join("other-exe");
        std::fs::copy(own_exe(), &copy).expect("copy exe");
        let sock = socket_path(&dir, Flavor::Dev).expect("path");
        let _server = spawn_helper(&copy, "serve_lock", &dir);
        let stream = UnixStream::connect(&sock).expect("connect");
        // The dev policy: on macOS this is the same-executable check, on Linux it always is.
        let err = verify_app_peer(&stream, &dir, Flavor::Dev, &PeerPolicy::native_dev())
            .expect_err("refused");
        assert!(matches!(err, PeerError::ExeMismatch(_)), "{err:?}");
    }

    #[test]
    fn inspect_own_parent_refuses_a_parent_that_is_not_a_browser() {
        // `cargo test` runs under cargo or nextest, never under Chrome.
        let err = inspect_own_parent(&PeerPolicy::release()).expect_err("refused");
        assert!(matches!(err, PeerError::ParentNotBrowser(_)), "{err:?}");
    }

    #[cfg(target_os = "macos")]
    #[test]
    fn macos_release_policy_checks_the_code_signature_of_the_peer() {
        // Both ends of a socketpair are this process, so the audit token is our own and it must
        // satisfy our own designated requirement. Release builds never take the relaxed path.
        let (a, _b) = UnixStream::pair().expect("pair");
        let err = verify_host_peer(&a, &PeerPolicy::release()).expect_err("parent is cargo");
        assert!(
            matches!(err, PeerError::ParentNotBrowser(_)),
            "own signature should pass and the parent check should fail: {err:?}"
        );
    }
}
