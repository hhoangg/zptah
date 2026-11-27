//! The real process runner against a fake `ardia` shell script. Unix only: the script is a `#!/bin/sh`
//! file made executable, which Windows can not run. The replay tests in `tests.rs` cover the
//! logic on every platform; these cover what only a real process shows (exit codes, a hang killed
//! by the timeout, where the command is looked up).
use super::runner::*;
use serde_json::json;
use std::os::unix::fs::PermissionsExt;
use std::path::Path;
use std::time::{Duration, Instant};

fn script(dir: &Path, body: &str) {
    let path = dir.join("ardia");
    std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
    std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o755)).unwrap();
}
fn cli(dir: &Path) -> Cli {
    let runner = ProcessRunner::with_search(format!("{}:/usr/bin:/bin", dir.display()), None);
    Cli::new(std::sync::Arc::new(runner))
}

#[test]
fn exit_codes_become_data_refusals_and_unreachable() {
    let dir = tempfile::tempdir().unwrap();
    script(
        dir.path(),
        r#"case "$*" in
  *"get url"*) echo '{"success":true,"data":{"url":"https://example.com/"},"error":null}'; exit 0;;
  *"stale"*) echo '{"success":false,"data":null,"error":"Stale ref @e19","code":"stale_ref"}'; exit 1;;
  *"call hello"*) echo "Control is off." >&2; exit 1;;
  *"call profiles.list"*) echo "ardia: Ardia isn't running" >&2; exit 2;;
  *"call settings.get"*) echo '{"warnBeforeQuitting":false}'; exit 0;;
  *) echo '{"success":false,"data":null,"error":"the ardia browse daemon closed the connection","code":"daemon_unreachable"}'; exit 2;;
esac"#,
    );
    let cli = cli(dir.path());
    let data = cli.browse("s", &["get", "url"]).unwrap();
    assert_eq!(data["url"], "https://example.com/");
    let refused = cli.browse("s", &["stale"]).unwrap_err();
    assert_eq!(refused.code(), Some("stale_ref"));
    assert!(matches!(
        cli.browse("s", &["anything"]),
        Err(Failure::Unreachable(_))
    ));
    assert_eq!(cli.call("hello", &json!({})), Err(Failure::ControlOff));
    assert!(matches!(
        cli.call("profiles.list", &json!({})),
        Err(Failure::Unreachable(_))
    ));
    assert_eq!(
        cli.call("settings.get", &json!({"keys": ["warnBeforeQuitting"]}))
            .unwrap()["warnBeforeQuitting"],
        false
    );
}

#[test]
fn the_arguments_reach_the_command_in_the_documented_order() {
    let dir = tempfile::tempdir().unwrap();
    let log = dir.path().join("argv.txt");
    script(
        dir.path(),
        &format!(
            r#"printf '%s\n' "$*" >> '{}'; echo '{{"ok":true,"results":[{{"op":"open","tabs":["tab-1"]}}]}}'"#,
            log.display()
        ),
    );
    let cli = cli(dir.path());
    assert_eq!(
        cli.tabs_open("https://example.com", "p-1").unwrap(),
        "tab-1"
    );
    cli.tabs_close("tab-1").unwrap();
    cli.call("tabs.hold", &json!({"session": "s"})).unwrap();
    let argv = std::fs::read_to_string(log).unwrap();
    let lines: Vec<&str> = argv.lines().collect();
    assert_eq!(
        lines[0],
        "tabs open https://example.com --profile p-1 --json"
    );
    assert_eq!(lines[1], "tabs close tab-1 --json");
    assert_eq!(
        lines[2],
        r#"call tabs.hold --params {"session":"s"} --json"#
    );
}

#[test]
fn a_hung_command_is_killed_when_the_timeout_ends() {
    let dir = tempfile::tempdir().unwrap();
    script(dir.path(), "sleep 30");
    let runner =
        ProcessRunner::with_search(format!("{}:/usr/bin:/bin", dir.path().display()), None);
    let started = Instant::now();
    let result = runner.run(&["call".into(), "hello".into()], Duration::from_millis(300));
    assert_eq!(result, Err(RunFailure::TimedOut));
    assert!(
        started.elapsed() < Duration::from_secs(5),
        "{:?}",
        started.elapsed()
    );
}

#[test]
fn ardia_is_found_on_the_path_and_in_the_local_bin_folder() {
    let home = tempfile::tempdir().unwrap();
    let local = home.path().join(".local").join("bin");
    std::fs::create_dir_all(&local).unwrap();
    script(&local, "echo hi");
    // Not on the PATH given, but in ~/.local/bin.
    let runner = ProcessRunner::with_search("/usr/bin:/bin", Some(home.path().to_path_buf()));
    assert_eq!(runner.executable(), Some(local.join("ardia")));
    // Not anywhere.
    let empty = tempfile::tempdir().unwrap();
    let runner = ProcessRunner::with_search("/usr/bin:/bin", Some(empty.path().to_path_buf()));
    assert_eq!(runner.executable(), None);
    assert_eq!(
        runner.run(&["call".into()], Duration::from_secs(1)),
        Err(RunFailure::NotFound)
    );
    // A folder on the PATH wins over ~/.local/bin.
    let first = tempfile::tempdir().unwrap();
    script(first.path(), "echo first");
    let runner = ProcessRunner::with_search(
        format!("{}:/usr/bin", first.path().display()),
        Some(home.path().to_path_buf()),
    );
    assert_eq!(runner.executable(), Some(first.path().join("ardia")));
}
