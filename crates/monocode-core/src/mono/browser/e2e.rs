//! The app side of the headless Chrome for Testing end-to-end test (spec 16.3). Debug builds
//! only, behind the `test-support` feature, unix only.
//!
//! One process plays both roles, because the peer checks require the native messaging host to be
//! the same executable as the app: started as `browser_e2e_app chrome-extension://<id>/` (by the
//! browser) it is the host, otherwise it is the app. The app runs the real link server and
//! manager with the dev flavor in a temporary socket directory, and the real
//! [`ExtensionUpstream`] read tools. The node runner drives it through line-delimited JSON on
//! stdin and stdout (`pair`, `wait`, `tool`, `quit`), so the scenarios live in
//! `extensions/zptah-browser/test/e2e/run.mjs`.
//!
//! Nothing here reads the user's keychain, browser profile or socket directory: keys and link
//! records are in memory, the socket directory is the one the runner passes, and the browser is
//! the runner's Chrome for Testing.
use super::gate::ExtensionUpstream;
use super::gate::Upstream;
use super::link::protocol::{AccessLevel, ChromeGroupColor, RosterEntry};
use super::links::keystore::MemoryKeyStore;
use super::links::manager::{
    Deps, LinkEvent, LinkManager, ManagerConfig, MemoryRecordStore, RosterSource,
};
use super::links::manifest_install::DEV_EXTENSION_ID;
use super::links::pairing::{pair_page_url, ProfileChoice};
use super::links::server::LinkServer;
use super::peer::{Flavor, PeerPolicy};
use serde_json::{json, Value};
use std::io::{BufRead, Write};
use std::path::PathBuf;
use std::sync::mpsc::{Receiver, RecvTimeoutError};
use std::sync::Arc;
use std::time::{Duration, Instant};

/// The one Mono the e2e runs as.
const MONO: &str = "0123456789abcdef0123456789abcdef";

struct OneMono;

impl RosterSource for OneMono {
    fn snapshot(&self) -> super::link::protocol::AuthResult {
        super::link::protocol::AuthResult {
            roster: vec![RosterEntry {
                id: MONO.into(),
                name: "e2e".into(),
                emoji: "E".into(),
                color: "#3366cc".into(),
                group_color: ChromeGroupColor::Blue,
                level: AccessLevel::Read,
                allowed_origins: Vec::new(),
                blocked_origins: Vec::new(),
                cursor: false,
            }],
            stopped: false,
        }
    }
}

/// Entry point of the example. `args` is argv without the program name. Returns the exit code.
pub fn main(args: &[String]) -> i32 {
    // Started by the browser as its native messaging host.
    if let Some(code) = super::host_mode::maybe_run(args) {
        return code;
    }
    match run_app(args) {
        Ok(()) => 0,
        Err(message) => {
            eprintln!("browser_e2e_app: {message}");
            1
        }
    }
}

fn flag(args: &[String], name: &str) -> Option<String> {
    args.iter()
        .position(|a| a == name)
        .and_then(|i| args.get(i + 1))
        .cloned()
}

fn run_app(args: &[String]) -> Result<(), String> {
    let socket_dir = PathBuf::from(flag(args, "--socket-dir").ok_or("--socket-dir is required")?);
    let browser = PathBuf::from(flag(args, "--browser-exe").ok_or("--browser-exe is required")?);
    if !socket_dir.is_absolute() || !browser.is_absolute() {
        return Err("--socket-dir and --browser-exe must be absolute paths".into());
    }
    let mut config = ManagerConfig::new(Flavor::Dev, "e2e");
    config.handshake_timeout = Duration::from_secs(20);
    let mut deps = Deps::new(
        Arc::new(MemoryKeyStore::new()),
        Arc::new(MemoryRecordStore::new()),
    );
    deps.roster = Arc::new(OneMono);
    let (manager, events) = LinkManager::new(config, deps);
    let policy = PeerPolicy::native_dev().with_extra_browser(browser);
    let _server = LinkServer::start(manager.clone(), &socket_dir, Flavor::Dev, policy)
        .map_err(|e| format!("cannot start the link server: {e}"))?;
    let upstream = {
        let manager = manager.clone();
        ExtensionUpstream::new(
            MONO,
            Arc::new(move || {
                let links = manager.links().ok()?;
                let live = links.into_iter().find(|l| l.connected)?;
                manager.handle(&live.instance_id)
            }),
        )
    };
    reply(&json!({"event": "ready", "mono": MONO, "extension_id": DEV_EXTENSION_ID}));

    let mut events = Events::new(events);
    let stdin = std::io::stdin();
    for line in stdin.lock().lines() {
        let line = line.map_err(|e| e.to_string())?;
        if line.trim().is_empty() {
            continue;
        }
        let cmd: Value = match serde_json::from_str(&line) {
            Ok(v) => v,
            Err(e) => {
                reply(&json!({"ok": false, "error": format!("bad command: {e}")}));
                continue;
            }
        };
        let id = cmd["id"].clone();
        let mut out = match cmd["cmd"].as_str().unwrap_or("") {
            "pair" => {
                let profile = ProfileChoice::new("Default", "e2e").expect("valid profile");
                let ticket = manager.start_pairing(profile);
                json!({"ok": true, "url": pair_page_url(DEV_EXTENSION_ID, &ticket.nonce)})
            }
            "wait" => {
                let kind = cmd["kind"].as_str().unwrap_or("");
                let timeout = Duration::from_millis(cmd["timeout_ms"].as_u64().unwrap_or(30_000));
                match events.wait_for(kind, timeout) {
                    Some(event) => json!({"ok": true, "event": event}),
                    None => {
                        json!({"ok": false, "error": format!("no {kind} event"), "seen": events.seen.clone()})
                    }
                }
            }
            "links" => match manager.links() {
                Ok(links) => json!({"ok": true, "links": links.iter().map(|l| json!({
                    "instance_id": l.instance_id, "name": l.name, "brand": l.brand, "connected": l.connected,
                })).collect::<Vec<_>>()}),
                Err(e) => json!({"ok": false, "error": e}),
            },
            "tool" => {
                let message = json!({"jsonrpc": "2.0", "id": 1, "method": "tools/call", "params": {
                    "name": cmd["name"], "arguments": cmd.get("args").cloned().unwrap_or_else(|| json!({})),
                }});
                match upstream.post("", None, &message) {
                    Ok((Some(reply), _)) => json!({"ok": true, "result": reply["result"]}),
                    Ok((None, _)) => json!({"ok": false, "error": "no reply"}),
                    Err(e) => json!({"ok": false, "error": e}),
                }
            }
            "quit" => {
                reply(&json!({"id": id, "ok": true}));
                return Ok(());
            }
            other => json!({"ok": false, "error": format!("unknown command {other}")}),
        };
        out["id"] = id;
        reply(&out);
    }
    Ok(())
}

fn reply(value: &Value) {
    let mut out = std::io::stdout().lock();
    let _ = writeln!(out, "{value}");
    let _ = out.flush();
}

/// Link events by kind, with the ones already seen kept so `wait` never misses one that arrived
/// before the command.
struct Events {
    rx: Receiver<LinkEvent>,
    pending: Vec<(String, Value)>,
    seen: Vec<String>,
}

impl Events {
    fn new(rx: Receiver<LinkEvent>) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            seen: Vec::new(),
        }
    }

    fn describe(event: &LinkEvent) -> (String, Value) {
        match event {
            LinkEvent::Paired { instance_id, name } => (
                "paired".into(),
                json!({"kind": "paired", "instance_id": instance_id, "name": name}),
            ),
            LinkEvent::Connected { instance_id, name } => (
                "connected".into(),
                json!({"kind": "connected", "instance_id": instance_id, "name": name}),
            ),
            LinkEvent::Disconnected { instance_id } => (
                "disconnected".into(),
                json!({"kind": "disconnected", "instance_id": instance_id}),
            ),
            LinkEvent::Refused { refusal, .. } => (
                "refused".into(),
                json!({"kind": "refused", "message": refusal.message()}),
            ),
            LinkEvent::Extension { envelope, .. } => (
                "extension".into(),
                json!({"kind": "extension", "event": format!("{:?}", envelope.method)}),
            ),
        }
    }

    fn wait_for(&mut self, kind: &str, timeout: Duration) -> Option<Value> {
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(i) = self.pending.iter().position(|(k, _)| k == kind) {
                return Some(self.pending.remove(i).1);
            }
            let left = deadline.saturating_duration_since(Instant::now());
            match self.rx.recv_timeout(left) {
                Ok(event) => {
                    let (k, v) = Self::describe(&event);
                    self.seen.push(k.clone());
                    self.pending.push((k, v));
                }
                Err(RecvTimeoutError::Timeout | RecvTimeoutError::Disconnected) => return None,
            }
        }
    }
}
