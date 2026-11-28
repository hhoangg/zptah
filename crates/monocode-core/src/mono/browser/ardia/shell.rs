//! Spotting a shell command that runs `ardia` itself, which skips the gate (design spec section 1,
//! decision 2). Claude's launch denies `Bash(ardia:*)` and Codex is told not to, neither is airtight,
//! so a command that does it is flagged in the audit.
use super::super::{AuditEntry, MonoBrowserStore};
use crate::tool::ToolCall;
use serde_json::Value;
use std::collections::HashSet;
use std::path::PathBuf;

/// Words that run the next word as a command.
const WRAPPERS: [&str; 13] = [
    "command", "exec", "sudo", "env", "time", "nohup", "nice", "builtin", "xargs", "stdbuf",
    "timeout", "npx", "pnpx",
];
const SHELLS: [&str; 5] = ["sh", "bash", "zsh", "dash", "fish"];

/// The options of a wrapper that take a value as the next word.
fn takes_value(wrapper: &str, option: &str) -> bool {
    let list: &[&str] = match wrapper {
        "env" => &["-u", "-C", "-S", "--unset", "--chdir"],
        "sudo" => &["-u", "-g", "-h", "-p", "-C", "-D", "-R", "-T", "-U"],
        "nice" => &["-n"],
        "xargs" => &["-n", "-P", "-I", "-L", "-s", "-d", "-E", "-a"],
        "stdbuf" => &["-i", "-o", "-e"],
        "timeout" => &["-k", "-s", "--signal", "--kill-after"],
        "npx" | "pnpx" => &["-p", "--package"],
        _ => &[],
    };
    list.contains(&option)
}

/// Whether the shell command `command` runs `ardia` as a program: at the start of a command
/// (after `;`, `&&`, `||`, `|`, `&`, a newline, `(`, `$(` or a backtick), after environment
/// assignments and wrappers like `command`, `sudo -u x`, `timeout 5`, `xargs` or `npx`, as
/// `node path/ardia`, inside `sh -c '...'` or inside `"$(...)"`. A word that only mentions it
/// (`grep ardia`, `echo ardia`, `cat ardia.txt`, `command -v ardia`, `which ardia`) does not count.
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
        || substitutions(command)
            .iter()
            .any(|inner| runs_ardia_depth(inner, depth + 1))
}

/// The commands inside `$(...)` and backticks that the shell would run, also inside double
/// quotes (never inside single quotes).
fn substitutions(line: &str) -> Vec<String> {
    let chars: Vec<char> = line.chars().collect();
    let mut out = Vec::new();
    let mut single = false;
    let mut double = false;
    let mut i = 0;
    while i < chars.len() {
        let c = chars[i];
        if c == '\\' {
            i += 2;
            continue;
        }
        if c == '"' && !single {
            double = !double;
        } else if c == '\'' && !double {
            // A single quote inside double quotes is a plain character.
            single = !single;
        } else if !single && c == '$' && chars.get(i + 1) == Some(&'(') {
            let mut depth = 1;
            let mut j = i + 2;
            while j < chars.len() && depth > 0 {
                match chars[j] {
                    '(' => depth += 1,
                    ')' => depth -= 1,
                    _ => {}
                }
                j += 1;
            }
            out.push(
                chars[i + 2..j.saturating_sub(1).max(i + 2)]
                    .iter()
                    .collect(),
            );
        } else if !single && c == '`' {
            if let Some(end) = chars[i + 1..].iter().position(|&x| x == '`') {
                out.push(chars[i + 1..i + 1 + end].iter().collect());
                i += end + 1;
            }
        }
        i += 1;
    }
    out
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

fn program_name(word: &str) -> &str {
    word.rsplit(['/', '\\'])
        .next()
        .unwrap_or(word)
        .trim_end_matches(".exe")
}

/// The words of one command (also what Codex sends: an argv array).
pub fn words_run_ardia(words: &[String], depth: usize) -> bool {
    let mut i = 0;
    while i < words.len() {
        let word = words[i].as_str();
        let assignment = word.split_once('=').is_some_and(|(name, _)| {
            !name.is_empty() && name.chars().all(|c| c.is_ascii_alphanumeric() || c == '_')
        });
        if assignment {
            i += 1;
            continue;
        }
        let wrapper = program_name(word);
        if !WRAPPERS.contains(&wrapper) {
            break;
        }
        i += 1;
        // The wrapper's options, with the values the ones that take one consume.
        while let Some(option) = words.get(i).filter(|w| w.starts_with('-') && w.len() > 1) {
            // `command -v ardia` only looks the program up.
            if wrapper == "command" && matches!(option.as_str(), "-v" | "-V") {
                return false;
            }
            // `env -S "ardia call hello"`: the value is a command line.
            if wrapper == "env" {
                let split = match option.as_str() {
                    "-S" | "--split-string" => words.get(i + 1).map(String::as_str),
                    o if o.starts_with("--split-string=") => o.strip_prefix("--split-string="),
                    o if o.starts_with("-S") && !o.starts_with("--") => Some(&o[2..]),
                    _ => None,
                };
                if split.is_some_and(|line| runs_ardia_depth(line, depth + 1)) {
                    return true;
                }
            }
            i += if takes_value(wrapper, option) { 2 } else { 1 };
        }
        // `timeout 5 cmd` has a duration before the command.
        if wrapper == "timeout" && words.get(i).is_some() {
            i += 1;
        }
    }
    let Some(program) = words.get(i) else {
        return false;
    };
    let name = program_name(program);
    if name == "ardia" {
        return true;
    }
    // `node path/ardia ...`
    if name == "node" {
        return words[i + 1..]
            .iter()
            .find(|w| !w.starts_with('-'))
            .is_some_and(|w| program_name(w) == "ardia");
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

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn env_split_string_is_a_command_line() {
        assert!(runs_ardia(r#"env -S "ardia call hello""#));
        assert!(runs_ardia(r#"env -i -S 'FOO=1 ardia call hello'"#));
        assert!(runs_ardia(r#"env --split-string="ardia call hello""#));
        assert!(!runs_ardia(r#"env -S "echo ardia""#));
        assert!(!runs_ardia(r#"env -S "grep ardia file""#));
    }

    #[test]
    fn a_single_quote_inside_double_quotes_is_a_plain_character() {
        assert!(runs_ardia(r#"echo "it's"; ardia call hello"#));
        assert!(runs_ardia(r#"echo "it's $(ardia call hello)""#));
        assert!(runs_ardia(r#"echo "it's" && echo "$(ardia call x)""#));
        assert!(!runs_ardia(r#"echo "it's ardia""#));
        assert!(!runs_ardia(r#"echo "it's"; echo 'ardia'"#));
        // Inside real single quotes a substitution is not run.
        assert!(!runs_ardia("echo '$(ardia call x)'"));
    }
}
