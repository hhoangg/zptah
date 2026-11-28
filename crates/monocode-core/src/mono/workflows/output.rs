//! Output of session nodes: the built-in fields, the hint that tells the agent how to record
//! the declared ones (`workflow.output`), the check of what it records, and the reading of a
//! fenced `workflow-output` block, which the fallback extraction answers with.
use super::model::{OutputField, OutputFieldType, SessionSurface};
use super::ordered::{Json, OrderedMap};
use super::template::{js_space, js_trim};

pub const OUTPUT_FENCE: &str = "workflow-output";

fn type_name(ty: OutputFieldType) -> &'static str {
    match ty {
        OutputFieldType::String => "string",
        OutputFieldType::Number => "number",
        OutputFieldType::Boolean => "boolean",
        OutputFieldType::List => "list",
        OutputFieldType::Object => "object",
    }
}

fn type_hint(ty: OutputFieldType) -> &'static str {
    match ty {
        OutputFieldType::String => "a JSON string",
        OutputFieldType::Number => "a JSON number",
        OutputFieldType::Boolean => "true or false",
        OutputFieldType::List => "a JSON array",
        OutputFieldType::Object => "a JSON object",
    }
}

fn describe_fields(fields: &OrderedMap<OutputField>) -> String {
    fields
        .iter()
        .map(|(name, field)| {
            let note = match field.description.as_deref() {
                Some(text) if !text.is_empty() => format!(": {text}"),
                _ => String::new(),
            };
            format!("- {name} ({}){note}", type_name(field.ty))
        })
        .collect::<Vec<_>>()
        .join("\n")
}

/// The fields every session node gets without declaring anything, and the type of each.
/// `reply` is the final reply text, so it exists only for chat sessions; the git fields come from
/// the node's worktree, so a cloud node has none (see `session_builtins`).
pub const SESSION_BUILTINS: [(&str, OutputFieldType); 7] = [
    ("reply", OutputFieldType::String),
    ("sessionId", OutputFieldType::String),
    ("worktree", OutputFieldType::String),
    ("branch", OutputFieldType::String),
    ("commit", OutputFieldType::String),
    ("changedFiles", OutputFieldType::List),
    ("diffStat", OutputFieldType::String),
];

/// The fields every command node gets.
pub const COMMAND_BUILTINS: [(&str, OutputFieldType); 3] = [
    ("exitCode", OutputFieldType::Number),
    ("stdout", OutputFieldType::String),
    ("stderr", OutputFieldType::String),
];

/// The session built-ins a cloud node can have: the app has no worktree of its own to look at.
const CLOUD_SESSION_BUILTINS: [&str; 2] = ["reply", "sessionId"];

/// The built-in output fields of a session node that is a chat or a terminal one, local or in
/// the cloud. A terminal session has no `reply` (no transcript the app can read), a cloud one
/// has no git fields.
pub fn session_builtins(surface: SessionSurface, cloud: bool) -> Vec<&'static str> {
    SESSION_BUILTINS
        .iter()
        .map(|(name, _)| *name)
        .filter(|name| surface == SessionSurface::Chat || *name != "reply")
        .filter(|name| !cloud || CLOUD_SESSION_BUILTINS.contains(name))
        .collect()
}

/// Whether `name` is a built-in field of any node type; a declared field may not use it.
pub fn is_builtin_name(name: &str) -> bool {
    SESSION_BUILTINS
        .iter()
        .chain(COMMAND_BUILTINS.iter())
        .any(|(builtin, _)| *builtin == name)
}

/// An example value of `ty`, for the command shown to the agent.
fn example_value(ty: OutputFieldType) -> &'static str {
    match ty {
        OutputFieldType::String => "\"...\"",
        OutputFieldType::Number => "0",
        OutputFieldType::Boolean => "true",
        OutputFieldType::List => "[]",
        OutputFieldType::Object => "{}",
    }
}

/// One short paragraph for the end of a session node's first prompt: the declared fields and the
/// exact command that records them. `cli` is the app executable, already quoted for a shell.
/// Empty when nothing is declared. It never asks for anything in the reply itself, so a user who
/// chats on in the session is not answered in JSON.
pub fn output_hint(fields: &OrderedMap<OutputField>, cli: &str) -> String {
    if fields.is_empty() {
        return String::new();
    }
    let names = describe_fields(fields);
    let example = fields
        .iter()
        .map(|(name, field)| format!("\"{name}\": {}", example_value(field.ty)))
        .collect::<Vec<_>>()
        .join(", ");
    format!(
        "This workflow step also needs these values from you, recorded by running a command (not written in your reply):\n{names}\nRun: {cli} app workflow.output '{{{example}}}'\nYou may run it again to correct a value; the last value of each field is used."
    )
}

/// Checks what `workflow.output` was given against the node's declared fields: an object, no
/// field the node does not declare, each value of its declared type. Any subset of the fields is
/// fine: the call may come in several parts. Err is the message for the caller.
pub fn check_output_value(
    fields: &OrderedMap<OutputField>,
    input: &Json,
) -> Result<OrderedMap<Json>, String> {
    if fields.is_empty() {
        return Err("This workflow step declares no output fields.".to_string());
    }
    let allowed = || fields.keys().cloned().collect::<Vec<_>>().join(", ");
    let Json::Object(given) = input else {
        return Err(format!(
            "Give a JSON object with the step's output fields: {}.",
            allowed()
        ));
    };
    if given.is_empty() {
        return Err(format!(
            "Give at least one of the step's output fields: {}.",
            allowed()
        ));
    }
    let mut checked = OrderedMap::new();
    for (name, value) in given.iter() {
        let Some(field) = fields.get(name) else {
            return Err(format!(
                "\"{name}\" is not an output field of this step; the fields are: {}.",
                allowed()
            ));
        };
        if !matches_type(value, field.ty) {
            return Err(format!(
                "Field \"{name}\" must be {} ({}).",
                type_hint(field.ty),
                type_name(field.ty)
            ));
        }
        checked.insert(name.clone(), value.clone());
    }
    Ok(checked)
}

/// The prompt of the hidden one-shot call that reads the declared fields out of a finished
/// session. `transcript` is already capped. The answer is read by `parse_workflow_output`.
pub fn extraction_prompt(fields: &OrderedMap<OutputField>, transcript: &str) -> String {
    [
        "Below is the transcript of a coding session. Read it and report the result fields listed here, using only what the transcript shows.".to_string(),
        describe_fields(fields),
        format!("Answer with exactly one fenced code block whose info string is `{OUTPUT_FENCE}`, containing a single JSON object with every one of these fields and nothing else. Do not run tools."),
        "Transcript:".to_string(),
        "<transcript>".to_string(),
        transcript.to_string(),
        "</transcript>".to_string(),
    ]
    .join("\n")
}

/// How much of a conversation the extraction call is shown, in characters.
pub const TRANSCRIPT_MAX_CHARS: usize = 60_000;
const OMITTED: &str = "[... the middle of the conversation is left out ...]";

/// The conversation the extraction call reads: what the user and the agent said, in order, one
/// labelled message after another (tool calls and reasoning are left out). When it is longer than
/// `max_chars`, the start (the step's own prompt) and the end (its conclusion) are kept.
pub fn transcript_text(rows: &[crate::transcript::Row], max_chars: usize) -> String {
    let mut messages: Vec<(&str, String)> = Vec::new();
    for row in rows {
        let label = match row.role.as_str() {
            "user" | "mono-system" => "User",
            "assistant" => "Assistant",
            _ => continue,
        };
        match messages.last_mut() {
            Some((last, text)) if *last == label && !row.first => text.push_str(&row.text),
            _ => messages.push((label, row.text.clone())),
        }
    }
    let text = messages
        .iter()
        .map(|(label, text)| format!("{label}: {}", text.trim()))
        .collect::<Vec<_>>()
        .join("\n\n");
    let total = text.chars().count();
    if total <= max_chars {
        return text;
    }
    let head_len = max_chars / 5;
    let tail_len = max_chars - head_len;
    let head: String = text.chars().take(head_len).collect();
    let tail: String = text.chars().skip(total - tail_len).collect();
    format!("{head}\n\n{OMITTED}\n\n{tail}")
}

/// `^ {0,3}(`{3,})([^`]*)$`: the backtick count and the info string of an opening fence.
fn open_fence(line: &str) -> Option<(usize, &str)> {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return None;
    }
    let info = rest.trim_start_matches('`');
    let ticks = rest.len() - info.len();
    (ticks >= 3 && !info.contains('`')).then_some((ticks, info))
}

/// `^ {0,3}`{n,}\s*$`: a line that closes a fence of `n` backticks.
fn closes_fence(line: &str, n: usize) -> bool {
    let rest = line.trim_start_matches(' ');
    if line.len() - rest.len() > 3 {
        return false;
    }
    let after = rest.trim_start_matches('`');
    rest.len() - after.len() >= n && after.chars().all(js_space)
}

/// The body of the last fenced block whose info string is exactly the output fence, or None.
fn last_output_block(reply: &str) -> Option<String> {
    // Split on `\r?\n`: a lone `\r` stays in the line.
    let count = reply.split('\n').count();
    let lines: Vec<&str> = reply
        .split('\n')
        .enumerate()
        .map(|(i, line)| {
            if i + 1 < count {
                line.strip_suffix('\r').unwrap_or(line)
            } else {
                line
            }
        })
        .collect();
    let mut found = None;
    let mut i = 0;
    while i < lines.len() {
        let Some((ticks, info)) = open_fence(lines[i]) else {
            i += 1;
            continue;
        };
        let is_output = js_trim(info) == OUTPUT_FENCE;
        let mut j = i + 1;
        while j < lines.len() && !closes_fence(lines[j], ticks) {
            j += 1;
        }
        // An unclosed fence runs to the end of the reply, as in Markdown.
        if is_output {
            found = Some(lines[i + 1..j].join("\n"));
        }
        i = j + 1;
    }
    found
}

fn matches_type(value: &Json, ty: OutputFieldType) -> bool {
    match ty {
        OutputFieldType::String => matches!(value, Json::String(_)),
        OutputFieldType::Number => value.as_f64().is_some(),
        OutputFieldType::Boolean => matches!(value, Json::Bool(_)),
        OutputFieldType::List => matches!(value, Json::Array(_)),
        OutputFieldType::Object => matches!(value, Json::Object(_)),
    }
}

/// Extracts and validates the `workflow-output` block from a session reply. The object keeps
/// the reply's key order, including fields the node did not declare. Err is the message for the
/// re-ask. No declared fields and no block is fine: the output is empty.
pub fn parse_workflow_output(
    reply: &str,
    fields: &OrderedMap<OutputField>,
) -> Result<OrderedMap<Json>, String> {
    let Some(body) = last_output_block(reply) else {
        if fields.is_empty() {
            return Ok(OrderedMap::new());
        }
        return Err(format!("No {OUTPUT_FENCE} block found in the reply."));
    };
    let Some(parsed) = Json::parse(&body) else {
        return Err(format!("The {OUTPUT_FENCE} block is not valid JSON."));
    };
    let Json::Object(value) = parsed else {
        return Err(format!(
            "The {OUTPUT_FENCE} block must contain a JSON object."
        ));
    };
    for (name, field) in fields.iter() {
        let ty = field.ty;
        let Some(found) = value.get(name) else {
            return Err(format!(
                "Field \"{name}\" is missing; expected {} ({}).",
                type_hint(ty),
                type_name(ty)
            ));
        };
        if !matches_type(found, ty) {
            return Err(format!(
                "Field \"{name}\" must be {} ({}).",
                type_hint(ty),
                type_name(ty)
            ));
        }
    }
    Ok(value)
}
