//! Runs the `ardia` command and reads what it prints.
//!
//! [`Runner`] is the only place a process is started, so every other part of the Ardia backend
//! takes a `dyn Runner` and its tests replay recorded replies (see `fixtures/`). [`Cli`] turns the
//! three reply shapes of section 12b of the design spec into one `Result`:
//!
//! * `ardia browse --session S <cmd> --json` prints `{success, data, error, code?, remedy?}`;
//! * `ardia call <method> --params J --json` prints the bare result;
//! * `ardia tabs open|close ... --json` prints `{ok, applied, results, tabs, ...}`.
//!
//! Exit codes: 0 ok, 1 refused or failed, 2 Ardia (or the browse daemon) can not be reached.
use serde_json::{json, Value};
use std::io::{BufRead, BufReader, Read};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::sync::{mpsc, Arc, OnceLock};
use std::time::{Duration, Instant};

pub const EXIT_REFUSED: i32 = 1;
pub const EXIT_UNREACHABLE: i32 = 2;
/// The longest an `ardia browse` command may take (a big page, a slow load).
pub const BROWSE_TIMEOUT: Duration = Duration::from_secs(60);
/// The longest a quick control call (`hello`, `tabs.hold`, `tabs close`) may take.
pub const QUICK_TIMEOUT: Duration = Duration::from_secs(15);
/// The string Ardia prints when its Control setting is off. Section 12b: not measured, the
/// fixture `call_control_off.stderr.txt` assumes it comes on stderr with exit 1.
const CONTROL_OFF: &str = "Control is off";

/// What a finished process printed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Output {
    pub code: i32,
    pub stdout: String,
    pub stderr: String,
}

/// Why no [`Output`] came back.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum RunFailure {
    /// There is no `ardia` command to run.
    NotFound,
    /// The process did not finish in time and was killed.
    TimedOut,
    Spawn(String),
}

/// A process whose stdout is read line by line (the event stream). Dropping it stops the process.
pub trait LineStream: Send {
    /// The next line, or `None` when the stream ended.
    fn next_line(&mut self) -> Option<String>;
}

/// Starts `ardia` processes. The real one is [`ProcessRunner`]; tests replay fixtures.
pub trait Runner: Send + Sync {
    /// Runs `ardia <args>` to its end, or kills it after `timeout`.
    fn run(&self, args: &[String], timeout: Duration) -> Result<Output, RunFailure>;
    /// Starts `ardia <args>` and reads its stdout as it comes.
    fn watch(&self, args: &[String]) -> Result<Box<dyn LineStream>, RunFailure>;
    /// Where the `ardia` command is, if it is installed.
    fn executable(&self) -> Option<PathBuf>;
}

// ---- the real process runner ----

/// The folders searched for `ardia`: the entries of `path`, then `~/.local/bin` (where Ardia's
/// "Command line" setting installs it).
pub fn search_dirs(path: &str, home: Option<&Path>) -> Vec<PathBuf> {
    let mut dirs: Vec<PathBuf> = std::env::split_paths(path)
        .filter(|d| !d.as_os_str().is_empty())
        .collect();
    if let Some(home) = home {
        let local = home.join(".local").join("bin");
        if !dirs.contains(&local) {
            dirs.push(local);
        }
    }
    dirs
}

/// The first executable file called `name` in `dirs`.
pub fn find_executable(dirs: &[PathBuf], name: &str) -> Option<PathBuf> {
    dirs.iter().find_map(|dir| {
        let plain = dir.join(name);
        #[cfg(windows)]
        for ext in ["exe", "cmd", "bat", "com"] {
            let candidate = plain.with_extension(ext);
            if is_executable(&candidate) {
                return Some(candidate);
            }
        }
        is_executable(&plain).then_some(plain)
    })
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path).is_ok_and(|m| m.is_file() && m.permissions().mode() & 0o111 != 0)
    }
    #[cfg(not(unix))]
    {
        path.is_file()
    }
}

/// Runs the real `ardia` command.
pub struct ProcessRunner {
    /// `Some`: the search path (tests). `None`: the login-shell PATH the harness gets.
    fixed_path: Option<String>,
    home: Option<PathBuf>,
    resolved: OnceLock<String>,
}
impl ProcessRunner {
    /// The app's runner: the login-shell PATH (what the harness gets) plus `~/.local/bin`.
    pub fn real() -> Self {
        Self {
            fixed_path: None,
            home: crate::scm::shell::dirs_home().map(PathBuf::from),
            resolved: OnceLock::new(),
        }
    }
    /// A runner that looks in `path` and `home/.local/bin` only, and touches no login shell.
    pub fn with_search(path: impl Into<String>, home: Option<PathBuf>) -> Self {
        Self {
            fixed_path: Some(path.into()),
            home,
            resolved: OnceLock::new(),
        }
    }
    fn path(&self) -> &str {
        match &self.fixed_path {
            Some(path) => path,
            None => self
                .resolved
                .get_or_init(crate::scm::shell::gui_search_path),
        }
    }
    fn command(&self, args: &[String]) -> Result<Command, RunFailure> {
        let exe = self.executable().ok_or(RunFailure::NotFound)?;
        let mut command = Command::new(exe);
        command.args(args).stdin(Stdio::null());
        if self.fixed_path.is_none() {
            crate::scm::shell::apply_gui_env(&mut command);
        }
        // The `ardia` script may need `node` from the same PATH.
        command.env("PATH", self.path());
        #[cfg(unix)]
        {
            use std::os::unix::process::CommandExt;
            // Its own process group, so a timeout kills the whole tree.
            command.process_group(0);
        }
        #[cfg(windows)]
        {
            use std::os::windows::process::CommandExt;
            command.creation_flags(0x0800_0000);
        }
        Ok(command)
    }
}
impl Runner for ProcessRunner {
    fn executable(&self) -> Option<PathBuf> {
        find_executable(&search_dirs(self.path(), self.home.as_deref()), "ardia")
    }

    fn run(&self, args: &[String], timeout: Duration) -> Result<Output, RunFailure> {
        let mut command = self.command(args)?;
        command.stdout(Stdio::piped()).stderr(Stdio::piped());
        let mut child = command
            .spawn()
            .map_err(|e| RunFailure::Spawn(e.to_string()))?;
        let out = drain(child.stdout.take());
        let err = drain(child.stderr.take());
        let deadline = Instant::now() + timeout;
        let status = loop {
            match child.try_wait() {
                Ok(Some(status)) => break status,
                Ok(None) if Instant::now() >= deadline => {
                    kill_tree(&mut child);
                    return Err(RunFailure::TimedOut);
                }
                Ok(None) => std::thread::sleep(Duration::from_millis(10)),
                Err(e) => return Err(RunFailure::Spawn(e.to_string())),
            }
        };
        // A grandchild that keeps the pipe open must not hold the call: take what is there.
        let grace = Duration::from_secs(2);
        Ok(Output {
            code: status.code().unwrap_or(-1),
            stdout: out.recv_timeout(grace).unwrap_or_default(),
            stderr: err.recv_timeout(grace).unwrap_or_default(),
        })
    }

    fn watch(&self, args: &[String]) -> Result<Box<dyn LineStream>, RunFailure> {
        let mut command = self.command(args)?;
        command.stdout(Stdio::piped()).stderr(Stdio::null());
        let mut child = command
            .spawn()
            .map_err(|e| RunFailure::Spawn(e.to_string()))?;
        let stdout = child
            .stdout
            .take()
            .ok_or_else(|| RunFailure::Spawn("no stdout".into()))?;
        Ok(Box::new(ChildStream {
            child,
            lines: BufReader::new(stdout),
        }))
    }
}

/// Reads a pipe to its end on a thread and sends the text once.
fn drain<R: Read + Send + 'static>(pipe: Option<R>) -> mpsc::Receiver<String> {
    let (sender, receiver) = mpsc::channel();
    std::thread::spawn(move || {
        let mut bytes = Vec::new();
        if let Some(mut pipe) = pipe {
            let _ = pipe.read_to_end(&mut bytes);
        }
        let _ = sender.send(String::from_utf8_lossy(&bytes).into_owned());
    });
    receiver
}

fn kill_tree(child: &mut Child) {
    #[cfg(unix)]
    // SAFETY: the child leads its own process group (`process_group(0)`).
    unsafe {
        libc::killpg(child.id() as libc::pid_t, libc::SIGKILL);
    }
    let _ = child.kill();
    let _ = child.wait();
}

struct ChildStream {
    child: Child,
    lines: BufReader<std::process::ChildStdout>,
}
impl LineStream for ChildStream {
    fn next_line(&mut self) -> Option<String> {
        let mut line = String::new();
        match self.lines.read_line(&mut line) {
            Ok(0) | Err(_) => None,
            Ok(_) => Some(line.trim_end().to_owned()),
        }
    }
}
impl Drop for ChildStream {
    fn drop(&mut self) {
        kill_tree(&mut self.child);
    }
}

// ---- reading the replies ----

/// Why an `ardia` call gave no data.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Failure {
    /// There is no `ardia` command.
    NotInstalled,
    /// Exit 2: Ardia, or the browse daemon, can not be reached.
    Unreachable(String),
    /// Ardia answered that its Control setting is off.
    ControlOff,
    /// Exit 1 or `success: false`: the command was refused or failed. `code` is Ardia's code
    /// (`tab_asleep`, `stale_ref`, ...).
    Refused {
        code: Option<String>,
        message: String,
    },
    TimedOut,
    /// The output was not the shape section 12b records.
    Malformed(String),
}
impl Failure {
    pub fn code(&self) -> Option<&str> {
        match self {
            Self::Refused { code, .. } => code.as_deref(),
            _ => None,
        }
    }
    /// A sentence for the tool error, never including page content beyond Ardia's own message.
    pub fn text(&self) -> String {
        match self {
            Self::NotInstalled => "The `ardia` command was not found.".into(),
            Self::Unreachable(_) => "Ardia can not be reached (it is not running).".into(),
            Self::ControlOff => "Control is off in Ardia.".into(),
            Self::Refused { message, .. } => clip(message, 400),
            Self::TimedOut => "Ardia did not answer in time.".into(),
            Self::Malformed(why) => format!("Ardia sent a reply zptah can not read ({why})."),
        }
    }
}
fn clip(text: &str, max: usize) -> String {
    text.chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .take(max)
        .collect()
}

/// The `ardia` command as the rest of the backend uses it.
#[derive(Clone)]
pub struct Cli {
    runner: Arc<dyn Runner>,
}
impl Cli {
    pub fn new(runner: Arc<dyn Runner>) -> Self {
        Self { runner }
    }
    pub fn runner(&self) -> &Arc<dyn Runner> {
        &self.runner
    }

    fn exec(&self, args: Vec<String>, timeout: Duration) -> Result<Output, Failure> {
        self.runner
            .run(&args, timeout)
            .map_err(|failure| match failure {
                RunFailure::NotFound => Failure::NotInstalled,
                RunFailure::TimedOut => Failure::TimedOut,
                RunFailure::Spawn(why) => Failure::Malformed(why),
            })
    }

    /// `ardia browse --session S <cmd...> --json`: the reply's `data`.
    pub fn browse(&self, session: &str, cmd: &[&str]) -> Result<Value, Failure> {
        self.browse_timeout(session, cmd, BROWSE_TIMEOUT)
    }
    pub fn browse_timeout(
        &self,
        session: &str,
        cmd: &[&str],
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let mut args: Vec<String> = ["browse", "--session", session]
            .into_iter()
            .map(String::from)
            .collect();
        args.extend(cmd.iter().map(|a| a.to_string()));
        args.push("--json".into());
        let out = self.exec(args, timeout)?;
        let reply: Option<Value> = serde_json::from_str(out.stdout.trim()).ok();
        let message = |reply: &Option<Value>| {
            reply
                .as_ref()
                .and_then(|r| r["error"].as_str())
                .map(str::to_owned)
                .filter(|m| !m.is_empty())
                .unwrap_or_else(|| first_line(&out.stderr, &out.stdout))
        };
        if out.code == EXIT_UNREACHABLE {
            return Err(Failure::Unreachable(message(&reply)));
        }
        let Some(reply) = reply else {
            return Err(if out.code == 0 {
                Failure::Malformed("not JSON".into())
            } else {
                Failure::Refused {
                    code: None,
                    message: message(&None),
                }
            });
        };
        if out.code == 0 && reply["success"] == true {
            return Ok(reply["data"].clone());
        }
        Err(Failure::Refused {
            code: reply["code"].as_str().map(str::to_owned),
            message: message(&Some(reply)),
        })
    }

    /// `ardia call <method> --params J --json`: the bare result.
    pub fn call(&self, method: &str, params: &Value) -> Result<Value, Failure> {
        self.call_timeout(method, params, QUICK_TIMEOUT)
    }
    pub fn call_timeout(
        &self,
        method: &str,
        params: &Value,
        timeout: Duration,
    ) -> Result<Value, Failure> {
        let args = vec![
            "call".to_owned(),
            method.to_owned(),
            "--params".to_owned(),
            params.to_string(),
            "--json".to_owned(),
        ];
        self.bare(args, timeout)
    }

    /// `ardia tabs open <url> --profile P --json`: the new tab's id (`results[0].tabs[0]`).
    pub fn tabs_open(&self, url: &str, profile: &str) -> Result<String, Failure> {
        let args = ["tabs", "open", url, "--profile", profile, "--json"]
            .into_iter()
            .map(String::from)
            .collect();
        let reply = self.bare(args, QUICK_TIMEOUT)?;
        reply["results"][0]["tabs"][0]
            .as_str()
            .map(str::to_owned)
            .ok_or_else(|| Failure::Malformed("no tab id in the open reply".into()))
    }
    /// `ardia tabs close <id> --json`.
    pub fn tabs_close(&self, id: &str) -> Result<(), Failure> {
        let args = ["tabs", "close", id, "--json"]
            .into_iter()
            .map(String::from)
            .collect();
        self.bare(args, QUICK_TIMEOUT).map(|_| ())
    }

    /// The commands whose reply is a bare JSON value (`call`, `tabs ...`).
    fn bare(&self, args: Vec<String>, timeout: Duration) -> Result<Value, Failure> {
        let out = self.exec(args, timeout)?;
        match out.code {
            0 => {
                let reply: Value = serde_json::from_str(out.stdout.trim())
                    .map_err(|_| Failure::Malformed("not JSON".into()))?;
                // `tabs open|close` answer `{ok: false, ...}` for a refused operation.
                if reply["ok"] == false {
                    return Err(Failure::Refused {
                        code: None,
                        message: first_line(&out.stderr, &out.stdout),
                    });
                }
                Ok(reply)
            }
            EXIT_UNREACHABLE => Err(Failure::Unreachable(first_line(&out.stderr, &out.stdout))),
            _ => {
                let text = format!("{}{}", out.stderr, out.stdout);
                if text.contains(CONTROL_OFF) {
                    Err(Failure::ControlOff)
                } else {
                    Err(Failure::Refused {
                        code: None,
                        message: first_line(&out.stderr, &out.stdout),
                    })
                }
            }
        }
    }
}

fn first_line(stderr: &str, stdout: &str) -> String {
    stderr
        .lines()
        .chain(stdout.lines())
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| clip(l, 300))
        .unwrap_or_default()
}

/// `{"session": s, ...}` helper for the hold and release calls.
pub fn session_params(session: &str, extra: Value) -> Value {
    let mut params = json!({"session": session});
    if let (Some(map), Some(extra)) = (params.as_object_mut(), extra.as_object()) {
        map.extend(extra.clone());
    }
    params
}
