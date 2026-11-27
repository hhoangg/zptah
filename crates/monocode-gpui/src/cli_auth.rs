//! Browser sign-in for a provider CLI (`claude mcp login`, `codex mcp login`, `opencode mcp auth`).
//!
//! These commands refuse to run without a terminal, and they keep a localhost listener open until
//! the browser redirects back. So the command runs on a pseudo-terminal and stays alive until it
//! exits, is cancelled or times out. The app opens the authorization URL itself: the child gets a
//! `BROWSER` variable (and `open` / `xdg-open` on its PATH on unix) that point at a shim which only
//! records the URL, and the output is scanned for a URL as a fallback.
//!
//! The runner is blocking and reports through a callback, so the page forwards events to the UI
//! thread and tests can capture them without opening a real browser. Call it from a dedicated
//! thread, not from an executor worker: those block signals, and a child inherits the mask.

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::atomic::{AtomicBool, Ordering},
    time::{Duration, Instant},
};

pub const SIGN_IN_TIMEOUT: Duration = Duration::from_secs(300);
/// How long the app waits for the CLI to open the browser itself before it does.
pub const FALLBACK_DELAY: Duration = Duration::from_secs(3);
const POLL: Duration = Duration::from_millis(50);
const MAX_OUTPUT: usize = 256 * 1024;
const DETAIL_LINES: usize = 40;
const BROWSER_FAILED: &str = "Browser launch failed";

/// What to run.
pub struct Spec {
    pub binary: PathBuf,
    pub args: Vec<String>,
    pub cwd: PathBuf,
    /// Provider id (`claude`, `codex`, ...), used for the fallback table and messages.
    pub provider: String,
    pub timeout: Duration,
    pub fallback_delay: Duration,
    /// Variables the child gets on top of the app's environment (an account's config dir).
    pub env: Vec<(String, String)>,
    /// Variables the child must not inherit. Honored on Unix only (the program runs under
    /// `env -u`); Windows can only set.
    pub remove: Vec<String>,
}

/// Progress reported while the command runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Event {
    /// The authorization URL became known (for the "Open browser" button).
    Url(String),
    /// The app should open this URL in the system browser now. Sent once per URL.
    Open(String),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Outcome {
    Success,
    Cancelled,
    TimedOut,
    /// The command could not start.
    Start(String),
    /// The command exited with an error. `output` is cleaned and redacted.
    Failed {
        code: Option<i32>,
        output: String,
    },
}

/// Providers whose CLI opens the browser through the OS by itself, so the app waits for its
/// "Browser launch failed" line instead of racing it.
fn opens_browser_natively(provider: &str) -> bool {
    provider == "codex"
}

/// Whether the app should open the URL itself now. `since_url` is how long the URL has been known.
pub fn fallback_due(
    provider: &str,
    since_url: Duration,
    delay: Duration,
    shim_called: bool,
    output: &str,
) -> bool {
    if shim_called {
        return false;
    }
    if opens_browser_natively(provider) {
        return output.contains(BROWSER_FAILED);
    }
    since_url >= delay
}

/// Everything the loop learns from the output and the shim log.
struct Progress {
    raw: Vec<u8>,
    text: String,
    url: Option<String>,
    url_since: Instant,
    opened: HashSet<String>,
    shim_called: bool,
    fallback_done: bool,
    log_len: usize,
}

impl Progress {
    fn new() -> Self {
        Self {
            raw: Vec::new(),
            text: String::new(),
            url: None,
            url_since: Instant::now(),
            opened: HashSet::new(),
            shim_called: false,
            fallback_done: false,
            log_len: 0,
        }
    }

    fn absorb(&mut self, chunk: &[u8], emit: &mut impl FnMut(Event)) {
        self.raw.extend_from_slice(chunk);
        if self.raw.len() > MAX_OUTPUT {
            let cut = self.raw.len() - MAX_OUTPUT;
            self.raw.drain(..cut);
        }
        self.text = String::from_utf8_lossy(&self.raw).into_owned();
        if self.url.is_none()
            && let Some(found) = extract_url(&self.text)
        {
            self.url = Some(found.clone());
            self.url_since = Instant::now();
            emit(Event::Url(found));
        }
    }

    fn shim_calls(&mut self, shims: Option<&Shims>, emit: &mut impl FnMut(Event)) {
        let Some(shims) = shims else { return };
        let (lines, len) = shims.new_calls(self.log_len);
        self.log_len = len;
        for line in lines {
            let Some(found) = first_http_url(&line) else {
                continue;
            };
            // cmd's `%*` cuts a URL at an unquoted `&`: prefer the full printed URL.
            let found = match &self.url {
                Some(printed) if printed.len() > found.len() && printed.starts_with(&found) => {
                    printed.clone()
                }
                _ => found,
            };
            if !looks_like_auth_url(&found) {
                continue;
            }
            self.shim_called = true;
            if self.opened.insert(found.clone()) {
                if self.url.as_deref() != Some(found.as_str()) {
                    self.url = Some(found.clone());
                    emit(Event::Url(found.clone()));
                }
                emit(Event::Open(found));
            }
        }
    }

    fn fallback(&mut self, spec: &Spec, emit: &mut impl FnMut(Event)) {
        if self.fallback_done {
            return;
        }
        let Some(found) = &self.url else { return };
        if fallback_due(
            &spec.provider,
            self.url_since.elapsed(),
            spec.fallback_delay,
            self.shim_called,
            &self.text,
        ) {
            self.fallback_done = true;
            if self.opened.insert(found.clone()) {
                emit(Event::Open(found.clone()));
            }
        }
    }
}

/// Runs the command until it exits. Blocks the calling thread; call it from a dedicated thread.
/// Setting `cancel` (from anywhere) kills the whole process tree.
pub fn run(spec: &Spec, cancel: &AtomicBool, mut emit: impl FnMut(Event)) -> Outcome {
    let shims = Shims::create();
    let mut proc = match Proc::start(spec, shims.as_ref()) {
        Ok(proc) => proc,
        Err(e) => return Outcome::Start(redact_urls(&e)),
    };
    let rx = proc.take_output();
    let started = Instant::now();
    let mut progress = Progress::new();
    let mut pending: Option<Vec<u8>> = None;
    let outcome = loop {
        while let Some(chunk) = pending.take().or_else(|| rx.try_recv().ok()) {
            progress.absorb(&chunk, &mut emit);
        }
        progress.shim_calls(shims.as_ref(), &mut emit);
        progress.fallback(spec, &mut emit);
        if let Some(code) = proc.poll() {
            // The reader can lag a little behind the exit; collect what is left.
            let end = Instant::now() + Duration::from_millis(300);
            while let Ok(chunk) = rx.recv_timeout(end.saturating_duration_since(Instant::now())) {
                progress.absorb(&chunk, &mut emit);
            }
            // A `$BROWSER` call right before exit.
            progress.shim_calls(shims.as_ref(), &mut emit);
            proc.kill_tree();
            let output = clean_output(&String::from_utf8_lossy(&progress.raw));
            break if code == Some(0) {
                Outcome::Success
            } else {
                Outcome::Failed { code, output }
            };
        }
        if cancel.load(Ordering::SeqCst) {
            proc.kill_tree();
            break Outcome::Cancelled;
        }
        if started.elapsed() >= spec.timeout {
            proc.kill_tree();
            break Outcome::TimedOut;
        }
        // Wait for output, or one poll interval.
        match rx.recv_timeout(POLL) {
            Ok(chunk) => pending = Some(chunk),
            Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => std::thread::sleep(POLL),
            Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {}
        }
    };
    drop(proc);
    outcome
}

/// One short line for the page, never the raw CLI output.
pub fn failure_message(name: &str, provider: &str, outcome: &Outcome) -> String {
    match outcome {
        Outcome::Success => format!("Signed in to \"{name}\"."),
        Outcome::Cancelled => "Sign-in cancelled.".into(),
        Outcome::TimedOut => format!("Sign-in for \"{name}\" timed out. Try again."),
        Outcome::Start(e) => {
            let lower = e.to_lowercase();
            if lower.contains("not installed") || lower.contains("no such file") {
                format!("The {provider} CLI is not installed, so \"{name}\" cannot sign in.")
            } else {
                format!("Could not start sign-in for \"{name}\".")
            }
        }
        Outcome::Failed { code, output } => {
            if output.to_lowercase().contains("awaiting approval") {
                format!(
                    "\"{name}\" from .mcp.json is awaiting approval. Use the Approve button on \
                     its row in this page, then sign in again."
                )
            } else {
                match code {
                    Some(code) => format!("Sign-in for \"{name}\" failed (exit {code})."),
                    None => format!("Sign-in for \"{name}\" failed."),
                }
            }
        }
    }
}

/// The text behind "Show details" for an outcome, if there is any.
pub fn details(outcome: &Outcome) -> Option<String> {
    match outcome {
        Outcome::Failed { output, .. } if !output.is_empty() => Some(output.clone()),
        Outcome::Start(e) if !e.is_empty() => Some(e.clone()),
        _ => None,
    }
}

/// The part of a URL after `://` that names the host, without any `user:pass@`.
fn authority(rest: &str) -> &str {
    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
    let authority = &rest[..end];
    authority
        .rsplit_once('@')
        .map_or(authority, |(_, host)| host)
}

/// `https://host/...` -> `host`, for showing where the browser is going without the secret query.
pub fn url_host(url: &str) -> String {
    let rest = url.split_once("://").map_or(url, |(_, rest)| rest);
    authority(rest).to_owned()
}

// ----- output handling -----

/// Removes escape sequences from terminal output. Cursor moves and erases that redraw a line
/// (spinners) become a `\r`, so the caller can keep only the final frame of a line.
pub fn strip_terminal(raw: &str) -> String {
    let mut out = String::with_capacity(raw.len());
    let mut chars = raw.chars().peekable();
    while let Some(c) = chars.next() {
        match c {
            '\x1b' => match chars.next() {
                Some('[') => {
                    let mut last = '\0';
                    for n in chars.by_ref() {
                        last = n;
                        if ('\x40'..='\x7e').contains(&n) {
                            break;
                        }
                    }
                    if matches!(last, 'D' | 'G' | 'H' | 'J' | 'K') {
                        out.push('\r');
                    }
                }
                Some(']') => {
                    // OSC: ends at BEL or ESC \.
                    while let Some(n) = chars.next() {
                        if n == '\x07' {
                            break;
                        }
                        if n == '\x1b' {
                            chars.next();
                            break;
                        }
                    }
                }
                Some('(' | ')' | '#') => {
                    chars.next();
                }
                _ => {}
            },
            '\r' | '\n' => out.push(c),
            '\t' => out.push(' '),
            c if c.is_control() => {}
            c => out.push(c),
        }
    }
    out
}

fn is_spinner_line(line: &str) -> bool {
    line.chars().all(|c| {
        c.is_whitespace()
            || matches!(
                c,
                '◒' | '◐' | '◓' | '◑' | '◇' | '◆' | '●' | '○' | '|' | '/' | '-' | '\\'
            )
            || ('\u{2800}'..='\u{28ff}').contains(&c)
    })
}

/// Drops `user:pass@` and replaces the query string (and fragment) of every URL with `?…`.
pub fn redact_urls(text: &str) -> String {
    text.split('\n')
        .map(|line| {
            line.split(' ')
                .map(|word| {
                    let Some(at) = word.find("://") else {
                        return word.to_owned();
                    };
                    let (head, rest) = word.split_at(at + 3);
                    let end = rest.find(['/', '?', '#']).unwrap_or(rest.len());
                    let host = authority(rest);
                    let tail = &rest[end..];
                    match tail.find(['?', '#']) {
                        Some(q) => format!("{head}{host}{}?…", &tail[..q]),
                        None => format!("{head}{host}{tail}"),
                    }
                })
                .collect::<Vec<_>>()
                .join(" ")
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// Terminal output made readable for the details view: no escapes, one line per redraw, repeated
/// and spinner-only lines dropped, URL queries redacted, the last lines only.
pub fn clean_output(raw: &str) -> String {
    let stripped = strip_terminal(raw);
    let mut lines: Vec<String> = Vec::new();
    for line in stripped.split('\n') {
        let frame = line
            .split('\r')
            .map(str::trim)
            .rfind(|f| !f.is_empty() && !is_spinner_line(f))
            .unwrap_or("");
        if frame.is_empty() || lines.last().is_some_and(|l| l == frame) {
            continue;
        }
        lines.push(frame.to_owned());
    }
    let start = lines.len().saturating_sub(DETAIL_LINES);
    redact_urls(&lines[start..].join("\n"))
}

fn local_host(url: &str) -> bool {
    let host = url_host(url);
    let host = host.split(':').next().unwrap_or("");
    matches!(host, "localhost" | "127.0.0.1" | "[::1]" | "0.0.0.0")
}

/// Whether a URL is an OAuth authorization request (not, say, the MCP server's own address).
fn looks_like_auth_url(url: &str) -> bool {
    let Some((_, query)) = url.split_once('?') else {
        return false;
    };
    query.split(['&', '#']).any(|pair| {
        let key = pair.split('=').next().unwrap_or("");
        matches!(
            key,
            "response_type" | "client_id" | "state" | "code_challenge"
        )
    })
}

fn trim_url(url: &str) -> &str {
    url.trim_end_matches([')', ']', '>', '.', ',', ';', '"', '\'', '\u{1b}'])
}

/// The first `http(s)` URL in a command-line string, as a shim received it.
fn first_http_url(text: &str) -> Option<String> {
    text.split_whitespace()
        .map(|w| w.trim_matches(['"', '\'']))
        .find(|w| {
            (w.starts_with("https://") || w.starts_with("http://")) && w.len() > "https://".len()
        })
        .map(|w| w.to_owned())
}

/// The authorization URL in terminal output: an OSC 8 hyperlink target, else the first
/// `https://` (or non-localhost `http://`) URL in the visible text that looks like an OAuth
/// request. A URL still being printed (no terminator after it yet) is not returned.
pub fn extract_url(raw: &str) -> Option<String> {
    let usable = |url: &str| {
        url.len() > "https://".len()
            && (url.starts_with("https://") || url.starts_with("http://"))
            && !local_host(url)
            && looks_like_auth_url(url)
    };
    let mut rest = raw;
    while let Some(at) = rest.find("\x1b]8;") {
        let after = &rest[at + 4..];
        let end = after.find(['\x07', '\x1b']);
        if let Some(end) = end
            && let Some((_, target)) = after[..end].split_once(';')
            && usable(target)
        {
            return Some(target.to_owned());
        }
        rest = &after[end.unwrap_or(after.len())..];
    }
    let text = strip_terminal(raw);
    let mut from = 0;
    while let Some(rel) = text[from..].find("http") {
        let start = from + rel;
        from = start + 4;
        let tail = &text[start..];
        if !(tail.starts_with("https://") || tail.starts_with("http://")) {
            continue;
        }
        let Some(end) = tail.find(char::is_whitespace) else {
            // Not finished printing.
            return None;
        };
        let url = trim_url(&tail[..end]);
        if usable(url) {
            return Some(url.to_owned());
        }
    }
    None
}

// ----- browser shims -----

/// A private directory with scripts that record their arguments to a file instead of launching a
/// browser: `open`, `xdg-open` and `browser` on unix, `browser.cmd` on Windows. Removed on drop.
struct Shims {
    dir: PathBuf,
    log: PathBuf,
}

impl Shims {
    fn create() -> Option<Self> {
        static COUNTER: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
        let nanos = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map_or(0, |d| d.as_nanos());
        let dir = std::env::temp_dir().join(format!(
            "zptah-auth-{}-{nanos}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        #[cfg(unix)]
        {
            use std::os::unix::fs::{DirBuilderExt, PermissionsExt};
            std::fs::DirBuilder::new().mode(0o700).create(&dir).ok()?;
            let log = dir.join("urls.log");
            let quoted = log.to_string_lossy().replace('\'', "'\\''");
            let script = format!("#!/bin/sh\nprintf '%s\\n' \"$*\" >> '{quoted}'\nexit 0\n");
            let shims = Self { dir, log };
            for name in ["open", "xdg-open", "browser"] {
                let path = shims.dir.join(name);
                std::fs::write(&path, &script).ok()?;
                std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).ok()?;
            }
            Some(shims)
        }
        #[cfg(not(unix))]
        {
            std::fs::create_dir(&dir).ok()?;
            let log = dir.join("urls.log");
            let script = format!(
                "@echo off\r\necho %*>> \"{}\"\r\nexit /b 0\r\n",
                log.display()
            );
            let shims = Self { dir, log };
            std::fs::write(shims.dir.join("browser.cmd"), script).ok()?;
            Some(shims)
        }
    }

    /// The program a `BROWSER` variable should name.
    fn browser(&self) -> PathBuf {
        self.dir
            .join(if cfg!(unix) { "browser" } else { "browser.cmd" })
    }

    /// Lines appended to the log after `seen` bytes, and the new length.
    fn new_calls(&self, seen: usize) -> (Vec<String>, usize) {
        let Ok(bytes) = std::fs::read(&self.log) else {
            return (Vec::new(), seen);
        };
        if bytes.len() <= seen {
            return (Vec::new(), seen);
        }
        // Only complete lines; a partial one is read on the next pass.
        let complete = bytes[seen..]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(0, |i| i + 1);
        let text = String::from_utf8_lossy(&bytes[seen..seen + complete]).into_owned();
        (text.lines().map(str::to_owned).collect(), seen + complete)
    }

    fn path_with(&self, path: &str) -> String {
        let mut dirs = vec![self.dir.clone()];
        dirs.extend(std::env::split_paths(path));
        std::env::join_paths(dirs)
            .map(|p| p.to_string_lossy().into_owned())
            .unwrap_or_else(|_| path.to_owned())
    }
}

impl Drop for Shims {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}

// ----- process -----

/// The environment of the child: what `cli::output` gives a CLI, plus a terminal and the shims.
fn child_env(shims: Option<&Shims>) -> Vec<(String, String)> {
    let mut env = monocode_core::scm::gui_env();
    let path = env
        .iter()
        .position(|(key, _)| key == "PATH")
        .map(|at| env.swap_remove(at).1)
        .unwrap_or_else(monocode_core::scm::terminal_search_path);
    env.extend([
        ("TERM".to_owned(), "xterm-256color".to_owned()),
        ("COLORTERM".to_owned(), "truecolor".to_owned()),
        ("TERM_PROGRAM".to_owned(), "zptah".to_owned()),
    ]);
    match shims {
        Some(shims) => {
            env.push(("PATH".into(), shims.path_with(&path)));
            env.push((
                "BROWSER".into(),
                shims.browser().to_string_lossy().into_owned(),
            ));
        }
        None => env.push(("PATH".into(), path)),
    }
    env
}

/// alacritty cannot remove a variable, so the program runs under `env -u KEY -- program`, which
/// then execs it (the pid is kept). Windows only sets: alacritty's `Options` has only an `env` map
/// (no removal), and an empty value is not the same as an unset variable for every CLI.
fn with_removals(program: String, args: Vec<String>, remove: &[String]) -> (String, Vec<String>) {
    if !cfg!(unix) || remove.is_empty() {
        return (program, args);
    }
    let mut wrapped = Vec::with_capacity(remove.len() * 2 + args.len() + 2);
    for key in remove {
        wrapped.push("-u".to_owned());
        wrapped.push(key.clone());
    }
    wrapped.push("--".into());
    wrapped.push(program);
    wrapped.extend(args);
    ("/usr/bin/env".into(), wrapped)
}

fn pty_options(spec: &Spec, shims: Option<&Shims>) -> alacritty_terminal::tty::Options {
    let (program, args) = with_removals(
        crate::terminal::shell::pty_program(&spec.binary.to_string_lossy()),
        spec.args.clone(),
        &spec.remove,
    );
    let mut env: std::collections::HashMap<String, String> = child_env(shims).into_iter().collect();
    env.extend(spec.env.iter().cloned());
    alacritty_terminal::tty::Options {
        shell: Some(alacritty_terminal::tty::Shell::new(program, args)),
        working_directory: Some(spec.cwd.clone()),
        drain_on_exit: false,
        env,
        #[cfg(target_os = "windows")]
        escape_args: true,
    }
}

fn pty_window() -> alacritty_terminal::event::WindowSize {
    alacritty_terminal::event::WindowSize {
        num_lines: 24,
        num_cols: 200,
        cell_width: 8,
        cell_height: 16,
    }
}

#[cfg(unix)]
struct Proc {
    pid: libc::pid_t,
    exited: Option<Option<i32>>,
    reader: Option<std::fs::File>,
    // Declared before the pty so the quit hook entry goes away first.
    _registration: monocode_core::process::Registration,
    // Dropping it sends SIGHUP and reaps the (already stopped) child.
    _pty: alacritty_terminal::tty::Pty,
}

#[cfg(unix)]
impl Proc {
    fn start(spec: &Spec, shims: Option<&Shims>) -> Result<Self, String> {
        use alacritty_terminal::tty;
        if !spec.cwd.is_dir() {
            return Err(format!("the folder {} does not exist", spec.cwd.display()));
        }
        let options = pty_options(spec, shims);
        // A GUI worker blocks signals, and exec keeps that mask: the CLI would never see SIGTERM.
        // The mask is cleared only around the spawn.
        let pty = {
            let mut empty: libc::sigset_t = unsafe { std::mem::zeroed() };
            let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut empty);
                libc::pthread_sigmask(libc::SIG_SETMASK, &empty, &mut old);
            }
            let pty = tty::new(&options, pty_window(), 0);
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
            pty.map_err(|e| e.to_string())?
        };
        let reader = pty.file().try_clone().map_err(|e| e.to_string())?;
        // alacritty opens the master non-blocking (it polls it); the clone shares that flag, and
        // the reader thread wants blocking reads so it never stops early and leaves output undrained.
        {
            use std::os::fd::AsRawFd;
            let fd = reader.as_raw_fd();
            unsafe {
                let flags = libc::fcntl(fd, libc::F_GETFL);
                if flags >= 0 {
                    libc::fcntl(fd, libc::F_SETFL, flags & !libc::O_NONBLOCK);
                }
            }
        }
        let id = pty.child().id();
        Ok(Self {
            pid: id as libc::pid_t,
            exited: None,
            reader: Some(reader),
            _registration: monocode_core::process::track(id),
            _pty: pty,
        })
    }

    /// Output chunks from the terminal, ending when the terminal closes.
    fn take_output(&mut self) -> std::sync::mpsc::Receiver<Vec<u8>> {
        use std::io::Read;
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(mut reader) = self.reader.take() {
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                while let Ok(n) = reader.read(&mut buf) {
                    if n == 0 || tx.send(buf[..n].to_vec()).is_err() {
                        break;
                    }
                }
            });
        }
        rx
    }

    /// `Some(exit code)` once the leader has exited (`Some(None)` when it died by a signal). The
    /// exit is observed without reaping, so the pid stays ours until the pty is dropped.
    fn poll(&mut self) -> Option<Option<i32>> {
        if self.exited.is_some() {
            return self.exited;
        }
        let mut info: libc::siginfo_t = unsafe { std::mem::zeroed() };
        let done = unsafe {
            libc::waitid(
                libc::P_PID,
                self.pid as libc::id_t,
                &mut info,
                libc::WEXITED | libc::WNOHANG | libc::WNOWAIT,
            )
        };
        if done == 0 && unsafe { info_pid(&info) } == self.pid {
            self.exited = Some(if info.si_code == libc::CLD_EXITED {
                Some(unsafe { info_status(&info) })
            } else {
                None
            });
        } else if done != 0 && std::io::Error::last_os_error().raw_os_error() == Some(libc::ECHILD)
        {
            // Not our child any more (reaped elsewhere): treat it as gone. Any other error
            // (EINTR) is retried on the next poll.
            self.exited = Some(None);
        }
        self.exited
    }

    /// SIGTERM to the whole group (the pty child leads its own session), SIGKILL after a grace.
    /// The leader is never reaped here, so its pid (and the group id) cannot be reused meanwhile.
    fn kill_tree(&mut self) {
        if self.exited.is_none() {
            unsafe { libc::kill(-self.pid, libc::SIGTERM) };
            let deadline = Instant::now() + Duration::from_millis(1500);
            while self.poll().is_none() && Instant::now() < deadline {
                std::thread::sleep(Duration::from_millis(20));
            }
        }
        unsafe { libc::kill(-self.pid, libc::SIGKILL) };
        let deadline = Instant::now() + Duration::from_secs(2);
        while self.poll().is_none() && Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

#[cfg(unix)]
unsafe fn info_pid(info: &libc::siginfo_t) -> libc::pid_t {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        info.si_pid()
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        info.si_pid
    }
}

#[cfg(unix)]
unsafe fn info_status(info: &libc::siginfo_t) -> i32 {
    #[cfg(any(target_os = "linux", target_os = "android"))]
    unsafe {
        info.si_status()
    }
    #[cfg(not(any(target_os = "linux", target_os = "android")))]
    {
        info.si_status
    }
}

/// ConPTY child. The pty moves to the reader thread, which also watches for the exit.
#[cfg(windows)]
struct Proc {
    pid: u32,
    exit: std::sync::Arc<std::sync::Mutex<Option<Option<i32>>>>,
    stop: std::sync::Arc<AtomicBool>,
    pty: Option<alacritty_terminal::tty::Pty>,
    _registration: monocode_core::process::Registration,
}

#[cfg(windows)]
impl Proc {
    fn start(spec: &Spec, shims: Option<&Shims>) -> Result<Self, String> {
        if !spec.cwd.is_dir() {
            return Err(format!("the folder {} does not exist", spec.cwd.display()));
        }
        let pty = alacritty_terminal::tty::new(&pty_options(spec, shims), pty_window(), 0)
            .map_err(|e| e.to_string())?;
        let pid = pty
            .child_watcher()
            .pid()
            .map(|pid| pid.get())
            .ok_or_else(|| "the sign-in command did not start".to_owned())?;
        Ok(Self {
            pid,
            exit: Default::default(),
            stop: Default::default(),
            pty: Some(pty),
            _registration: monocode_core::process::track(pid),
        })
    }

    fn take_output(&mut self) -> std::sync::mpsc::Receiver<Vec<u8>> {
        use alacritty_terminal::tty::{ChildEvent, EventedReadWrite};
        let (tx, rx) = std::sync::mpsc::channel();
        if let Some(mut pty) = self.pty.take() {
            let exit = self.exit.clone();
            let stop = self.stop.clone();
            std::thread::spawn(move || {
                let mut buf = [0u8; 4096];
                loop {
                    let n = pty.reader().try_read(&mut buf);
                    if n > 0 {
                        if tx.send(buf[..n].to_vec()).is_err() {
                            break;
                        }
                        continue;
                    }
                    if let Ok(ChildEvent::Exited(status)) =
                        pty.child_watcher().event_rx().try_recv()
                    {
                        // Keep reading until the output has been quiet for a moment.
                        let mut last = Instant::now();
                        while last.elapsed() < Duration::from_millis(300) {
                            let n = pty.reader().try_read(&mut buf);
                            if n == 0 {
                                std::thread::sleep(Duration::from_millis(15));
                            } else if tx.send(buf[..n].to_vec()).is_err() {
                                break;
                            } else {
                                last = Instant::now();
                            }
                        }
                        *exit.lock().unwrap_or_else(|e| e.into_inner()) =
                            Some(status.and_then(|s| s.code()));
                        break;
                    }
                    if stop.load(Ordering::SeqCst) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(15));
                }
            });
        }
        rx
    }

    fn poll(&mut self) -> Option<Option<i32>> {
        *self.exit.lock().unwrap_or_else(|e| e.into_inner())
    }

    /// `taskkill /T /F` ends the whole tree; the reader thread then drops the pty.
    fn kill_tree(&mut self) {
        use std::os::windows::process::CommandExt;
        let mut kill = std::process::Command::new("taskkill");
        kill.args(["/PID", &self.pid.to_string(), "/T", "/F"])
            .creation_flags(0x0800_0000);
        let _ = kill.status();
        self.stop.store(true, Ordering::SeqCst);
        self.exit
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_or_insert(None);
    }
}

#[cfg(windows)]
impl Drop for Proc {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::SeqCst);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const CLAUDE: &str = "Starting authentication for \"supabase\"\u{2026}\r\n\
If the browser didn't open, visit:\r\n\
\x1b]8;;https://api.supabase.com/v1/oauth/authorize?client_id=abc&state=s1&redirect_uri=http://localhost:59796/callback\x1b\\https://api.supabase.com/v1/oauth/authorize?client_id=abc&state=s1&redirect_uri=http://localhost:59796/callback\x1b]8;;\x1b\\\r\n\
Waiting for authorization\u{2026} (^C to cancel)\r\n";

    const OPENCODE: &str = "\x1b[?25l\x1b[999D\x1b[J\u{25d2}  Starting\r\n\
\x1b[999D\x1b[J\u{25d0}  Starting\r\n\
\x1b[999D\x1b[J\u{25c7}  Authorize in your browser:\r\n\
\u{25cf}  https://mcp.example.test/authorize?response_type=code&state=zz\r\n\
\u{2502}  Waiting for authorization\r\n";

    const CODEX: &str = "Authorize `linear` by opening this URL in your browser:\n\
https://linear.example.test/oauth/authorize?client_id=c&state=q\n\n\
(Browser launch failed; please copy the URL above manually.)\n";

    #[test]
    fn extracts_osc8_target_first() {
        assert_eq!(
            extract_url(CLAUDE).as_deref(),
            Some(
                "https://api.supabase.com/v1/oauth/authorize?client_id=abc&state=s1&redirect_uri=http://localhost:59796/callback"
            )
        );
    }

    #[test]
    fn extracts_plain_text_url_without_osc8() {
        let plain = strip_terminal(CLAUDE);
        // Feed text that has no escapes at all.
        assert!(
            extract_url(&plain)
                .unwrap()
                .starts_with("https://api.supabase.com/")
        );
    }

    #[test]
    fn extracts_opencode_url_through_spinner_redraws() {
        assert_eq!(
            extract_url(OPENCODE).as_deref(),
            Some("https://mcp.example.test/authorize?response_type=code&state=zz")
        );
    }

    #[test]
    fn extracts_codex_url() {
        assert_eq!(
            extract_url(CODEX).as_deref(),
            Some("https://linear.example.test/oauth/authorize?client_id=c&state=q")
        );
    }

    #[test]
    fn ignores_localhost_and_unfinished_urls() {
        assert_eq!(
            extract_url("listening on http://localhost:1234/callback?state=1\n"),
            None
        );
        assert_eq!(
            extract_url("visit https://auth.example.test/authorize?sta"),
            None
        );
        // The MCP server's own address is not an authorization request.
        assert_eq!(
            extract_url("Starting authentication for x (https://mcp.example.test/mcp)\n"),
            None
        );
        assert_eq!(
            extract_url(
                "see http://localhost:1/x?state=1 then http://auth.example.test/a?state=2 \n"
            )
            .as_deref(),
            Some("http://auth.example.test/a?state=2")
        );
    }

    #[test]
    fn first_http_url_reads_shim_arguments() {
        assert_eq!(
            first_http_url("--new-window https://a.example.test/x?y=1").as_deref(),
            Some("https://a.example.test/x?y=1")
        );
        assert_eq!(first_http_url("file:///etc/passwd"), None);
        assert_eq!(first_http_url("javascript:alert(1)"), None);
    }

    #[test]
    fn cleans_output_for_details() {
        let cleaned = clean_output(OPENCODE);
        assert!(!cleaned.contains('\x1b'));
        assert!(!cleaned.contains("Starting\nStarting"));
        assert!(cleaned.contains("Authorize in your browser:"));
        assert!(cleaned.contains("https://mcp.example.test/authorize?\u{2026}"));
        assert!(!cleaned.contains("state=zz"));
        let claude = clean_output(CLAUDE);
        assert!(!claude.contains("client_id"));
        assert!(claude.contains("Waiting for authorization"));
    }

    #[test]
    fn keeps_only_the_last_lines() {
        let raw: String = (0..100).map(|i| format!("line {i}\n")).collect();
        let cleaned = clean_output(&raw);
        assert_eq!(cleaned.lines().count(), DETAIL_LINES);
        assert!(cleaned.ends_with("line 99"));
    }

    #[test]
    fn redacts_query_and_fragment() {
        assert_eq!(
            redact_urls("go to https://a.test/p?token=s#frag now"),
            "go to https://a.test/p?\u{2026} now"
        );
        assert_eq!(redact_urls("no url? here"), "no url? here");
    }

    #[test]
    fn host_and_redaction_drop_userinfo() {
        assert_eq!(
            url_host("https://user:pw@auth.example.test/x"),
            "auth.example.test"
        );
        assert_eq!(
            redact_urls("at https://user:pw@auth.example.test/x?a=1"),
            "at https://auth.example.test/x?\u{2026}"
        );
        assert!(!redact_urls("https://user:pw@auth.example.test/x").contains("pw"));
    }

    #[test]
    fn host_hides_path_and_query() {
        assert_eq!(
            url_host("https://api.supabase.com/v1/x?y=1"),
            "api.supabase.com"
        );
        assert_eq!(url_host("http://example.test:8080?x"), "example.test:8080");
    }

    #[test]
    fn maps_failures_to_one_line() {
        let failed = |output: &str| Outcome::Failed {
            code: Some(1),
            output: output.into(),
        };
        let approval = failure_message(
            "supabase",
            "claude",
            &failed("\"supabase\" is from .mcp.json and awaiting approval. Run `claude`"),
        );
        assert!(approval.contains("awaiting approval"));
        assert!(approval.contains("Approve button"));
        assert_eq!(
            failure_message("x", "claude", &failed("boom")),
            "Sign-in for \"x\" failed (exit 1)."
        );
        assert_eq!(
            failure_message(
                "x",
                "claude",
                &Outcome::Failed {
                    code: None,
                    output: String::new()
                }
            ),
            "Sign-in for \"x\" failed."
        );
        assert_eq!(
            failure_message("x", "claude", &Outcome::Cancelled),
            "Sign-in cancelled."
        );
        assert!(failure_message("x", "claude", &Outcome::TimedOut).contains("timed out"));
        assert!(
            failure_message(
                "x",
                "codex",
                &Outcome::Start("codex CLI not installed".into())
            )
            .contains("not installed")
        );
        assert!(!failure_message("x", "codex", &Outcome::Start("boom".into())).contains("boom"));
    }

    #[test]
    fn fallback_decision_table() {
        let delay = Duration::from_secs(3);
        let late = Duration::from_secs(4);
        let early = Duration::from_millis(500);
        let due = |provider, since, shim, out| fallback_due(provider, since, delay, shim, out);
        // The shim already delivered the URL: never open again.
        assert!(!due("claude", late, true, ""));
        assert!(!due("codex", late, true, BROWSER_FAILED));
        // Providers that rely on BROWSER / PATH shims: open after the delay.
        assert!(!due("claude", early, false, ""));
        assert!(due("claude", late, false, ""));
        assert!(due("opencode", late, false, ""));
        // Codex opens the browser itself: only when it says it could not.
        assert!(!due("codex", late, false, "Authorize `x`"));
        assert!(due("codex", early, false, CODEX));
    }

    #[cfg(unix)]
    mod pty {
        use super::super::*;
        use std::{os::unix::fs::PermissionsExt, path::Path, sync::Arc};

        struct Fake {
            dir: PathBuf,
            script: PathBuf,
        }

        impl Fake {
            fn new(body: &str) -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "zptah-fake-cli-{}-{}",
                    std::process::id(),
                    std::time::SystemTime::now()
                        .duration_since(std::time::UNIX_EPOCH)
                        .unwrap()
                        .as_nanos()
                ));
                std::fs::create_dir_all(&dir).unwrap();
                let script = dir.join("fake-cli");
                std::fs::write(
                    &script,
                    format!(
                        "#!/bin/sh\n[ -t 0 ] && [ -t 1 ] || {{ echo \"not a tty\" >&2; exit 3; }}\n{body}\n"
                    ),
                )
                .unwrap();
                std::fs::set_permissions(&script, std::fs::Permissions::from_mode(0o755)).unwrap();
                Self { dir, script }
            }

            fn spec(&self, timeout: Duration) -> Spec {
                Spec {
                    binary: self.script.clone(),
                    args: vec![],
                    cwd: self.dir.clone(),
                    provider: "claude".into(),
                    timeout,
                    fallback_delay: Duration::from_millis(300),
                    env: vec![],
                    remove: vec![],
                }
            }
        }

        impl Drop for Fake {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.dir);
            }
        }

        fn alive(pid: i32) -> bool {
            unsafe { libc::kill(pid, 0) == 0 }
        }

        #[test]
        fn runs_on_a_tty_and_captures_the_shim_url_once() {
            let fake = Fake::new(
                "echo 'visit https://auth.example.test/authorize?state=x'\n\
                 \"$BROWSER\" 'https://auth.example.test/authorize?state=x'\n\
                 exit 0",
            );
            let cancel = Arc::new(AtomicBool::new(false));
            let mut events = Vec::new();
            let outcome = run(&fake.spec(Duration::from_secs(20)), &cancel, |e| {
                events.push(e)
            });
            assert_eq!(outcome, Outcome::Success);
            let opens = events
                .iter()
                .filter(|e| matches!(e, Event::Open(_)))
                .collect::<Vec<_>>();
            assert_eq!(
                opens,
                vec![&Event::Open(
                    "https://auth.example.test/authorize?state=x".into()
                )]
            );
            assert!(events.contains(&Event::Url(
                "https://auth.example.test/authorize?state=x".into()
            )));
        }

        #[test]
        fn open_and_xdg_open_on_path_are_shimmed_too() {
            let fake = Fake::new(
                "open 'https://auth.example.test/a?state=1'\n\
                 xdg-open 'https://auth.example.test/a?state=1'\n\
                 exit 0",
            );
            let cancel = AtomicBool::new(false);
            let mut opens = Vec::new();
            let outcome = run(&fake.spec(Duration::from_secs(20)), &cancel, |e| {
                if let Event::Open(url) = e {
                    opens.push(url);
                }
            });
            assert_eq!(outcome, Outcome::Success);
            assert_eq!(
                opens,
                vec!["https://auth.example.test/a?state=1".to_owned()]
            );
        }

        #[test]
        fn fallback_opens_the_printed_url_when_the_cli_does_not() {
            let fake = Fake::new(
                "echo 'visit https://auth.example.test/fb?state=2'\n\
                 sleep 2\n\
                 exit 0",
            );
            let cancel = AtomicBool::new(false);
            let mut opens = Vec::new();
            let outcome = run(&fake.spec(Duration::from_secs(30)), &cancel, |e| {
                if let Event::Open(url) = e {
                    opens.push(url);
                }
            });
            assert_eq!(outcome, Outcome::Success);
            assert_eq!(
                opens,
                vec!["https://auth.example.test/fb?state=2".to_owned()]
            );
        }

        #[test]
        fn failure_keeps_cleaned_output_and_the_exit_code() {
            let fake = Fake::new("echo 'it broke https://x.example.test/a?token=secret'\nexit 7");
            let cancel = AtomicBool::new(false);
            let outcome = run(&fake.spec(Duration::from_secs(20)), &cancel, |_| {});
            let Outcome::Failed { code, output } = outcome else {
                panic!("expected failure");
            };
            assert_eq!(code, Some(7));
            assert!(output.contains("it broke"));
            assert!(!output.contains("secret"));
        }

        #[test]
        fn cancel_kills_the_process_tree() {
            let pidfile = std::env::temp_dir().join(format!("zptah-pid-{}", std::process::id()));
            let fake = Fake::new(&format!(
                "sleep 300 &\necho $! > '{}'\necho $$ >> '{}'\nwait",
                pidfile.display(),
                pidfile.display()
            ));
            let cancel = Arc::new(AtomicBool::new(false));
            let flag = cancel.clone();
            let watch = pidfile.clone();
            let canceller = std::thread::spawn(move || {
                let end = Instant::now() + Duration::from_secs(10);
                while Instant::now() < end {
                    if std::fs::read_to_string(&watch).is_ok_and(|t| t.lines().count() == 2) {
                        break;
                    }
                    std::thread::sleep(Duration::from_millis(20));
                }
                flag.store(true, Ordering::SeqCst);
            });
            let began = Instant::now();
            let outcome = run(&fake.spec(Duration::from_secs(60)), &cancel, |_| {});
            canceller.join().unwrap();
            assert_eq!(outcome, Outcome::Cancelled);
            assert!(began.elapsed() < Duration::from_secs(15));
            let pids: Vec<i32> = std::fs::read_to_string(&pidfile)
                .unwrap()
                .lines()
                .map(|l| l.trim().parse().unwrap())
                .collect();
            let _ = std::fs::remove_file(&pidfile);
            assert_eq!(pids.len(), 2);
            for pid in pids {
                let end = Instant::now() + Duration::from_secs(5);
                while alive(pid) && Instant::now() < end {
                    std::thread::sleep(Duration::from_millis(20));
                }
                assert!(!alive(pid), "process {pid} survived the cancel");
            }
        }

        #[test]
        fn the_child_does_not_inherit_a_blocked_signal_mask() {
            // A GUI worker blocks signals; the CLI must still die from SIGTERM.
            let fake = Fake::new("kill -TERM $$\necho survived\nexit 0");
            let cancel = AtomicBool::new(false);
            let mut set: libc::sigset_t = unsafe { std::mem::zeroed() };
            let mut old: libc::sigset_t = unsafe { std::mem::zeroed() };
            unsafe {
                libc::sigemptyset(&mut set);
                libc::sigaddset(&mut set, libc::SIGTERM);
                libc::pthread_sigmask(libc::SIG_BLOCK, &set, &mut old);
            }
            let outcome = run(&fake.spec(Duration::from_secs(20)), &cancel, |_| {});
            unsafe { libc::pthread_sigmask(libc::SIG_SETMASK, &old, std::ptr::null_mut()) };
            let Outcome::Failed { code, output } = outcome else {
                panic!("expected the child to be killed, got {outcome:?}");
            };
            assert_eq!(code, None);
            assert!(!output.contains("survived"));
        }

        #[test]
        fn a_browser_call_right_before_exit_is_not_lost() {
            let fake = Fake::new("\"$BROWSER\" 'https://auth.example.test/z?state=9'\nexit 0");
            let cancel = AtomicBool::new(false);
            let mut opens = Vec::new();
            let outcome = run(&fake.spec(Duration::from_secs(20)), &cancel, |e| {
                if let Event::Open(url) = e {
                    opens.push(url);
                }
            });
            assert_eq!(outcome, Outcome::Success);
            assert_eq!(
                opens,
                vec!["https://auth.example.test/z?state=9".to_owned()]
            );
        }

        #[test]
        fn timeout_kills_and_reports_timed_out() {
            let fake = Fake::new("sleep 300");
            let cancel = AtomicBool::new(false);
            let began = Instant::now();
            let outcome = run(&fake.spec(Duration::from_millis(300)), &cancel, |_| {});
            assert_eq!(outcome, Outcome::TimedOut);
            assert!(began.elapsed() < Duration::from_secs(10));
        }

        #[test]
        fn a_missing_binary_is_a_start_failure() {
            let cancel = AtomicBool::new(false);
            let spec = Spec {
                binary: PathBuf::from("/nonexistent/zptah-cli"),
                args: vec![],
                cwd: std::env::temp_dir(),
                provider: "claude".into(),
                timeout: Duration::from_secs(5),
                fallback_delay: FALLBACK_DELAY,
                env: vec![],
                remove: vec![],
            };
            let outcome = run(&spec, &cancel, |_| {});
            assert!(matches!(outcome, Outcome::Start(_)), "{outcome:?}");
        }

        #[test]
        fn the_account_environment_is_set_and_stray_variables_are_removed() {
            let fake = Fake::new(
                "printf '%s|%s|%s\\n' \"${CLAUDE_CONFIG_DIR-unset}\" \"${ZPTAH_STRAY-unset}\" \"$1\" > \"$0.env\"\nexit 0",
            );
            // Not a variable of this process: the removal list must still win over what the app
            // process would pass on, which is simulated by a value the spec sets and removes.
            let mut spec = fake.spec(Duration::from_secs(20));
            spec.args = vec!["with space".into()];
            spec.env = vec![
                ("CLAUDE_CONFIG_DIR".into(), "/accounts/work".into()),
                ("ZPTAH_STRAY".into(), "set-but-removed".into()),
            ];
            spec.remove = vec!["ZPTAH_STRAY".into()];
            let cancel = AtomicBool::new(false);
            assert_eq!(run(&spec, &cancel, |_| {}), Outcome::Success);
            let seen = std::fs::read_to_string(fake.dir.join("fake-cli.env")).unwrap();
            assert_eq!(seen.trim(), "/accounts/work|unset|with space");
        }

        #[test]
        fn removals_wrap_the_program_in_env_on_unix_only() {
            let (program, args) = with_removals("/bin/x".into(), vec!["a".into()], &[]);
            assert_eq!((program.as_str(), args.len()), ("/bin/x", 1));
            let (program, args) = with_removals(
                "/bin/x".into(),
                vec!["a".into()],
                &["K1".into(), "K2".into()],
            );
            assert_eq!(program, "/usr/bin/env");
            assert_eq!(args, ["-u", "K1", "-u", "K2", "--", "/bin/x", "a"]);
        }

        #[test]
        fn the_shim_dir_is_removed_after_the_run() {
            let fake = Fake::new("echo \"$BROWSER\" > \"$0.browser\"\nexit 0");
            let cancel = AtomicBool::new(false);
            assert_eq!(
                run(&fake.spec(Duration::from_secs(20)), &cancel, |_| {}),
                Outcome::Success
            );
            let browser = std::fs::read_to_string(fake.dir.join("fake-cli.browser")).unwrap();
            assert!(!Path::new(browser.trim()).exists());
            assert!(!Path::new(browser.trim()).parent().unwrap().exists());
        }
    }
}
