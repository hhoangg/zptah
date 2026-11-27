//! Spotting a shell command that runs `ardia` itself, which skips the gate (design spec section 1,
//! decision 2). Claude's launch denies `Bash(ardia:*)` and Codex is told not to, neither is airtight,
//! so a command that does it is flagged in the audit.
use super::super::{AuditEntry, MonoBrowserStore};
use crate::tool::ToolCall;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

/// Words that run the next word as a command.
const WRAPPERS: [&str; 8] = [
    "command", "exec", "sudo", "env", "time", "nohup", "nice", "builtin",
];
const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "fish"];

/// Whether the shell command `command` runs `ardia` as a program: at the start of a command
/// (after `;`, `&&`, `||`, `|`, `&`, a newline, `(`, `$(` or a backtick), after environment
/// assignments and wrappers like `command` or `sudo`, or inside `sh -c '...'`. A word that only
/// mentions it (`grep ardia`, `echo ardia`, `cat ardia.txt`) does not count.
pub fn runs_ardia(command: &str) -> bool {
    runs_ardia_depth(command, 0)
}

fn runs_ardia_depth(command: &str, depth: usize) -> bool {
    if depth > 3 {
        return false;
    }
    split_commands(command)
        .iter()
        .any(|segment| segment_runs_ardia(segment, depth))
}

/// The commands of a shell line, split at the unquoted separators.
fn split_commands(line: &str) -> Vec<String> {
    let mut out = Vec::new();
    let mut current = String::new();
    let mut quote: Option<char> = None;
    let mut chars = line.chars().peekable();
    while let Some(c) = chars.next() {
        match quote {
            Some(q) => {
                current.push(c);
                if c == q {
                    quote = None;
                } else if c == '\\' && q == '"' {
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
            }
            None => match c {
                '\'' | '"' => {
                    quote = Some(c);
                    current.push(c);
                }
                '\\' => {
                    current.push(c);
                    if let Some(next) = chars.next() {
                        current.push(next);
                    }
                }
                ';' | '|' | '&' | '\n' | '(' | ')' | '`' | '{' | '}' => {
                    out.push(std::mem::take(&mut current));
                }
                _ => current.push(c),
            },
        }
    }
    out.push(current);
    out.into_iter().filter(|s| !s.trim().is_empty()).collect()
}

fn segment_runs_ardia(segment: &str, depth: usize) -> bool {
    let words = shlex::split(segment)
        .unwrap_or_else(|| segment.split_whitespace().map(str::to_owned).collect());
    words_run_ardia(&words, depth)
}

/// The words of one command (also what Codex sends: an argv array).
pub fn words_run_ardia(words: &[String], depth: usize) -> bool {
    let mut i = 0;
    while i < words.len() {
        let word = words[i].as_str();
        let assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        let wrapper_flag =
            i > 0 && word.starts_with('-') && WRAPPERS.contains(&words[i - 1].as_str());
        if assignment || WRAPPERS.contains(&word) || wrapper_flag {
            i += 1;
            continue;
        }
        break;
    }
    let Some(program) = words.get(i) else {
        return false;
    };
    let name = program
        .rsplit(['/', '\\'])
        .next()
        .unwrap_or(program)
        .trim_end_matches(".exe");
    if name == "ardia" {
        return true;
    }
    if SHELLS.contains(&name) {
        // `sh -c 'ardia ...'`, `bash -lc "..."`.
        let rest = &words[i + 1..];
        if let Some(at) = rest
            .iter()
            .position(|w| w.starts_with('-') && !w.starts_with("--") && w.ends_with('c'))
        {
            if let Some(script) = rest.get(at + 1) {
                return runs_ardia_depth(script, depth + 1);
            }
        }
    }
    false
}

/// The command a shell tool call runs: `command` as a string or an argv array.
fn command_of(input: &Value) -> Option<Vec<String>> {
    match &input["command"] {
        Value::String(text) => Some(vec![text.clone()]),
        Value::Array(words) => Some(
            words
                .iter()
                .filter_map(|w| w.as_str().map(str::to_owned))
                .collect(),
        ),
        _ => None,
    }
}

/// Whether a tool call of a harness (`Bash`, `commandExecution`, `shell`, ...) with JSON `input`
/// runs `ardia`.
pub fn input_runs_ardia(tool_name: &str, input: &str) -> bool {
    let lower = tool_name.to_lowercase();
    if !(lower.contains("bash") || lower.contains("shell") || lower.contains("exec")) {
        return false;
    }
    let Ok(input) = serde_json::from_str::<Value>(input) else {
        return false;
    };
    match command_of(&input) {
        Some(parts) if parts.len() == 1 => runs_ardia(&parts[0]),
        Some(words) => words_run_ardia(&words, 0),
        None => false,
    }
}

/// The audit row for such a call.
pub fn entry(session_id: i64) -> AuditEntry {
    AuditEntry {
        ts_ms: crate::time::now_ms(),
        session_id,
        tool: "shell_ardia".to_owned(),
        url: String::new(),
        summary: "ran the ardia command from the shell: it skips the browser gate".to_owned(),
    }
}

/// Watches the tool calls of a Mono's run and records the ones that run `ardia` in a shell.
pub struct ShellAudit {
    target: Option<(String, PathBuf)>,
    session: i64,
    seen: HashSet<String>,
}
impl ShellAudit {
    /// For the run of `mono` (with a store to write to) in app session `session`.
    pub fn new(mono: Option<&crate::agent::MonoContext>, session: Option<i64>) -> Self {
        Self {
            target: mono.and_then(|m| Some((m.mono.id.clone(), m.browser_database.clone()?))),
            session: session.unwrap_or(0),
            seen: HashSet::new(),
        }
    }
    /// A tool call (it is seen again as its input streams in; it is recorded once).
    pub fn see(&mut self, tool: &ToolCall) {
        let Some((mono_id, database)) = &self.target else {
            return;
        };
        if self.seen.contains(&tool.id) || !input_runs_ardia(&tool.name, &tool.input) {
            return;
        }
        self.seen.insert(tool.id.clone());
        let _ = MonoBrowserStore::at(database.clone()).record(mono_id, &entry(self.session));
    }
}
