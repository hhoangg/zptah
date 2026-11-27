//! What `flow show` says about a flow, read into plain types, and the values a flow call puts on
//! the command line (design spec sections 7 and 12b, "Flows" and "Arguments").
//!
//! Measured on Ardia 0.7.0 (fixtures `flow_*.json`): `data.flow` has `version`, `name`,
//! `description`, `site`, `origin`, `startUrl`, `hosts`, `createdAt`, `createdBy`, `params`,
//! `output`, `verified`, `steps[] {seq, command[], url, ok, requests, action {type, intent,
//! candidates, fingerprint, value}}`, `kind` and `writes`.
//!
//! NOT measured, so every reader here fails closed (an unknown shape is an error or a risk, never
//! a pass): the shape of `params` (an array of names or objects with a `name`, or a map), `kind`
//! values other than `actions`, the `requests[]` entries of a data flow, how a step names the
//! parameter it uses, and the `--fields`, `--where` and `--limit` options of `flow run`.
use crate::mono::browser::ardia::runner::{literal_value, plain_value};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};
use std::collections::BTreeMap;

/// The most parameters one run passes.
pub const MAX_PARAMS: usize = 20;
/// The longest value of one parameter.
pub const MAX_PARAM_VALUE: usize = 2000;
/// The longest flow name Ardia is given.
pub const MAX_NAME: usize = 64;

// ---- names and values that reach the command line ----

/// A flow name: `^[A-Za-z0-9][A-Za-z0-9._-]{0,63}$`. Never a leading `-`, never a `/`, a space or
/// a `..` path (the name is a file name in Ardia's flow store).
pub fn flow_name(raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= MAX_NAME
        && bytes[0].is_ascii_alphanumeric()
        && bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
    if ok {
        Ok(raw.to_owned())
    } else {
        Err(format!(
            "`name` is a flow name: letters, digits, `.`, `_` and `-`, starting with a letter or digit, at most {MAX_NAME} characters."
        ))
    }
}

/// A parameter name: `^[A-Za-z_][A-Za-z0-9_]{0,63}$`. A `k=v` token made from it can never be
/// read as an option, whatever the value is.
pub fn param_key(raw: &str) -> Result<String, String> {
    let bytes = raw.as_bytes();
    let ok = !bytes.is_empty()
        && bytes.len() <= 64
        && (bytes[0].is_ascii_alphabetic() || bytes[0] == b'_')
        && bytes
            .iter()
            .all(|c| c.is_ascii_alphanumeric() || *c == b'_');
    if ok {
        Ok(raw.to_owned())
    } else {
        Err(format!(
            "`{}` is not a parameter name (letters, digits and `_`, not starting with a digit).",
            raw.chars().take(40).collect::<String>()
        ))
    }
}

/// The parameters of a `flow_run` call: `params` is an object of text, number or boolean values.
pub fn parse_params(args: &Value) -> Result<BTreeMap<String, String>, String> {
    let params = &args["params"];
    if params.is_null() {
        return Ok(BTreeMap::new());
    }
    let map = params
        .as_object()
        .ok_or("`params` is an object: parameter name to value.")?;
    if map.len() > MAX_PARAMS {
        return Err(format!("A flow takes at most {MAX_PARAMS} parameters."));
    }
    let mut out = BTreeMap::new();
    for (key, value) in map {
        param_key(key)?;
        let text = match value {
            Value::String(s) => s.clone(),
            Value::Number(n) => n.to_string(),
            Value::Bool(b) => b.to_string(),
            _ => return Err(format!("The value of parameter `{key}` is text.")),
        };
        // The value travels inside one `key=value` token, which starts with a letter: whatever it
        // says (`--yes`, `-5`) is data, never an option. Only what an argument can not carry is
        // refused.
        literal_value(&format!("params.{key}"), &text, true, MAX_PARAM_VALUE)?;
        out.insert(key.clone(), text);
    }
    Ok(out)
}

/// The output options of a data flow (not measured: Ardia's docs name `--fields`, `--where` and
/// `--limit`; their values are checked here and a refusal comes back from Ardia).
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct RunOptions {
    pub limit: Option<u32>,
    pub fields: Vec<String>,
    pub filter: Option<String>,
}
impl RunOptions {
    pub fn is_empty(&self) -> bool {
        self.limit.is_none() && self.fields.is_empty() && self.filter.is_none()
    }
    pub fn parse(args: &Value) -> Result<Self, String> {
        let mut out = Self::default();
        if !args["limit"].is_null() {
            let n = args["limit"]
                .as_u64()
                .filter(|n| (1..=1000).contains(n))
                .ok_or("`limit` is a whole number from 1 to 1000.")?;
            out.limit = Some(n as u32);
        }
        if !args["fields"].is_null() {
            let items = args["fields"]
                .as_array()
                .filter(|a| !a.is_empty() && a.len() <= 30)
                .ok_or("`fields` is a list of up to 30 field names.")?;
            for item in items {
                let name = item.as_str().ok_or("`fields` holds field names (text).")?;
                let ok = !name.is_empty()
                    && name.len() <= 64
                    && name.as_bytes()[0].is_ascii_alphanumeric()
                    && name
                        .bytes()
                        .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'.' | b'_' | b'-'));
                if !ok {
                    return Err(format!(
                        "`{}` is not a field name.",
                        name.chars().take(40).collect::<String>()
                    ));
                }
                out.fields.push(name.to_owned());
            }
        }
        if !args["where"].is_null() {
            let text = args["where"].as_str().ok_or("`where` is text.")?;
            plain_value("where", text.trim())?;
            if text.len() > 200 {
                return Err("`where` is too long.".into());
            }
            out.filter = Some(text.trim().to_owned());
        }
        Ok(out)
    }
}

/// The argv of `flow run`: `(command and options, values)`. Every option comes before the `--`,
/// the name after it; `--yes` only when the caller says the gate's own check passed for a flow
/// that writes.
pub fn run_argv(
    name: &str,
    yes: bool,
    params: &BTreeMap<String, String>,
    opts: &RunOptions,
) -> (Vec<String>, Vec<String>) {
    let mut cmd: Vec<String> = vec!["flow".into(), "run".into()];
    if yes {
        cmd.push("--yes".into());
    }
    for (key, value) in params {
        cmd.push("--param".into());
        cmd.push(format!("{key}={value}"));
    }
    if let Some(limit) = opts.limit {
        cmd.push("--limit".into());
        cmd.push(limit.to_string());
    }
    if !opts.fields.is_empty() {
        cmd.push("--fields".into());
        cmd.push(opts.fields.join(","));
    }
    if let Some(filter) = &opts.filter {
        cmd.push("--where".into());
        cmd.push(filter.clone());
    }
    (cmd, vec![name.to_owned()])
}

// ---- the flow ----

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Kind {
    /// `kind` absent: a data flow (GET requests only).
    Data,
    Actions,
    /// A kind this version does not know.
    Other(String),
}

/// What one step does, from its `command` (the first word of an `ardia browse` command, or for
/// `find <by> <value> <verb>` the verb), else from `action.type`.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum StepKind {
    Open(String),
    Click,
    Check,
    Hover,
    Fill,
    Type,
    Select,
    Press(String),
    Scroll,
    Wait,
    History,
    Read,
    FillOtp,
    /// `eval`, `script`, `cdp` and the like: code runs in the page or the browser.
    Script(String),
    Unknown(String),
}

#[derive(Clone, Debug)]
pub struct Step {
    pub seq: Option<i64>,
    pub command: Vec<String>,
    pub url: String,
    pub kind: StepKind,
    pub intent: String,
    /// Every name the step knows its target by (fingerprint, candidates, `--name`).
    pub names: Vec<String>,
    pub role: String,
    /// The text a fill or type puts in (a literal, or a placeholder of a parameter).
    pub value: Option<String>,
    /// The command and `action.type` do not describe the same kind of action.
    pub mismatch: bool,
    /// The step carries a value Ardia redacted when it recorded it (a typed password).
    pub redacted: bool,
    /// The methods of the requests the step recorded (`GET`, `POST`, ...).
    pub methods: Vec<String>,
}

#[derive(Clone, Debug)]
pub struct Flow {
    /// `data.flow` as Ardia gave it: the hash covers all of it.
    pub raw: Value,
    pub name: String,
    pub site: String,
    pub description: String,
    pub start_url: String,
    pub params: Vec<String>,
    pub kind: Kind,
    pub writes: Option<bool>,
    pub verified: Option<bool>,
    pub steps: Vec<Step>,
}
impl Flow {
    /// A flow that only reads: no `kind` (or `data`) and an explicit `writes: false`.
    pub fn is_data(&self) -> bool {
        self.kind == Kind::Data && self.writes == Some(false)
    }
}

fn text(v: &Value) -> String {
    v.as_str().unwrap_or("").to_owned()
}

pub fn param_names(v: &Value) -> Result<Vec<String>, String> {
    match v {
        Value::Null => Ok(Vec::new()),
        Value::Array(items) => items
            .iter()
            .map(|item| match item {
                Value::String(s) => Ok(s.clone()),
                Value::Object(o) => o
                    .get("name")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                    .ok_or_else(|| "a parameter has no name".to_owned()),
                _ => Err("a parameter is not a name".to_owned()),
            })
            .collect(),
        Value::Object(map) => Ok(map.keys().cloned().collect()),
        _ => Err("`params` is not a list".to_owned()),
    }
}

const FIND_BY_NAME: [&str; 7] = [
    "text",
    "label",
    "placeholder",
    "title",
    "alt",
    "testid",
    "name",
];

fn kind_of(command: &[String], action_type: &str) -> StepKind {
    let word = |i: usize| command.get(i).map(|w| w.to_lowercase()).unwrap_or_default();
    let from_verb = |verb: &str, at: usize| -> StepKind {
        match verb {
            "click" | "dblclick" => StepKind::Click,
            "check" | "uncheck" => StepKind::Check,
            "hover" => StepKind::Hover,
            "fill" => StepKind::Fill,
            "type" => StepKind::Type,
            "select" => StepKind::Select,
            "press" | "key" => StepKind::Press(command.get(at + 1).cloned().unwrap_or_default()),
            other => StepKind::Unknown(format!("find {other}")),
        }
    };
    let head = word(0);
    match head.as_str() {
        "" => match action_type.to_lowercase().as_str() {
            "open" | "navigate" => StepKind::Open(String::new()),
            "click" => StepKind::Click,
            "check" | "uncheck" => StepKind::Check,
            "hover" => StepKind::Hover,
            "fill" => StepKind::Fill,
            "type" => StepKind::Type,
            "select" => StepKind::Select,
            "press" => StepKind::Press(String::new()),
            other => StepKind::Unknown(other.to_owned()),
        },
        "open" | "goto" | "navigate" | "visit" => {
            StepKind::Open(command.get(1).cloned().unwrap_or_default())
        }
        "find" => from_verb(&word(3), 3),
        "click" | "dblclick" => StepKind::Click,
        "check" | "uncheck" => StepKind::Check,
        "hover" => StepKind::Hover,
        "fill" => StepKind::Fill,
        "type" => StepKind::Type,
        "select" => StepKind::Select,
        "press" | "key" => StepKind::Press(command.get(1).cloned().unwrap_or_default()),
        "scroll" | "scrollintoview" | "scrollinto" => StepKind::Scroll,
        "wait" => StepKind::Wait,
        "back" | "forward" | "reload" | "refresh" => StepKind::History,
        "snapshot" | "snap" | "read" | "get" | "screenshot" | "console" | "errors" | "text" => {
            StepKind::Read
        }
        "fill-otp" => StepKind::FillOtp,
        "eval" | "evaluate" | "js" | "script" | "addinitscript" | "removeinitscript" | "cdp"
        | "batch" | "pushstate" => StepKind::Script(head),
        other => StepKind::Unknown(other.to_owned()),
    }
}

/// The family of a kind, to tell whether `command` and `action.type` agree.
fn family(kind: &StepKind) -> &'static str {
    match kind {
        StepKind::Open(_) => "open",
        StepKind::Click | StepKind::Check | StepKind::Hover => "click",
        StepKind::Fill | StepKind::Type => "fill",
        StepKind::Select => "select",
        StepKind::Press(_) => "press",
        _ => "other",
    }
}

fn parse_step(v: &Value) -> Result<Step, String> {
    let obj = v.as_object().ok_or("a step is not an object")?;
    let command: Vec<String> = match obj.get("command") {
        None | Some(Value::Null) => Vec::new(),
        Some(Value::Array(items)) => items
            .iter()
            .map(|c| {
                c.as_str()
                    .map(str::to_owned)
                    .ok_or("a command word is not text")
            })
            .collect::<Result<_, _>>()?,
        Some(_) => return Err("a step's command is not a list".into()),
    };
    let action = v.get("action").cloned().unwrap_or(Value::Null);
    let action_type = text(&action["type"]);
    let mut kind = kind_of(&command, &action_type);
    if let StepKind::Open(url) = &mut kind {
        if url.is_empty() {
            *url = text(&action["value"]);
        }
    }
    // The command decides; `action.type` is checked against it.
    let by_type = kind_of(&[], &action_type);
    let mismatch = !action_type.is_empty()
        && !command.is_empty()
        && family(&kind) != "other"
        && family(&by_type) != "other"
        && family(&kind) != family(&by_type);
    let mut names: Vec<String> = Vec::new();
    let mut add = |n: &str| {
        let n = n.trim();
        if !n.is_empty() && !names.iter().any(|x| x == n) && names.len() < 6 {
            names.push(n.to_owned());
        }
    };
    if let Some(n) = action["fingerprint"]["name"].as_str() {
        add(n);
    }
    let mut role = text(&action["fingerprint"]["role"]);
    for cand in action["candidates"].as_array().into_iter().flatten() {
        if let Some(n) = cand["name"].as_str() {
            add(n);
        }
        let by = cand["by"].as_str().unwrap_or("");
        if FIND_BY_NAME.contains(&by) {
            if let Some(n) = cand["value"].as_str() {
                add(n);
            }
        }
        if by == "role" && role.is_empty() {
            role = text(&cand["value"]);
        }
    }
    if command.first().map(|c| c.as_str()) == Some("find") {
        let by = command.get(1).map(|b| b.to_lowercase()).unwrap_or_default();
        if FIND_BY_NAME.contains(&by.as_str()) {
            if let Some(n) = command.get(2) {
                add(n);
            }
        }
        if by == "role" && role.is_empty() {
            role = command.get(2).cloned().unwrap_or_default();
        }
    }
    if let Some(at) = command.iter().position(|c| c == "--name") {
        if let Some(n) = command.get(at + 1) {
            add(n);
        }
    }
    let value = match &kind {
        StepKind::Fill | StepKind::Type => {
            let at = if command.first().map(|c| c.as_str()) == Some("find") {
                4
            } else {
                2
            };
            command
                .get(at)
                .cloned()
                .or_else(|| action["value"].as_str().map(str::to_owned))
        }
        _ => action["value"].as_str().map(str::to_owned),
    };
    let redacted = command
        .iter()
        .map(String::as_str)
        .chain(value.as_deref())
        .any(|t| t.to_lowercase().contains("<redacted>"));
    let methods = v["requests"]
        .as_array()
        .into_iter()
        .flatten()
        .map(|r| r["method"].as_str().unwrap_or("?").to_uppercase())
        .collect();
    Ok(Step {
        seq: obj.get("seq").and_then(Value::as_i64),
        command,
        url: text(&v["url"]),
        kind,
        intent: text(&action["intent"]),
        names,
        role,
        value,
        mismatch,
        redacted,
        methods,
    })
}

/// Reads the `data` of `flow show <wanted>`. `Err` is a flow zptah can not read: it is not run.
pub fn parse_flow(data: &Value, wanted: &str) -> Result<Flow, String> {
    let raw = data
        .get("flow")
        .filter(|f| f.is_object())
        .ok_or("no `flow` object")?;
    let name = text(&raw["name"]);
    if name.is_empty() {
        return Err("the flow has no name".into());
    }
    if name != wanted {
        return Err("Ardia answered for another flow".into());
    }
    let kind = match &raw["kind"] {
        Value::Null => Kind::Data,
        Value::String(s) if s == "data" => Kind::Data,
        Value::String(s) if s == "actions" => Kind::Actions,
        Value::String(s) => Kind::Other(s.clone()),
        _ => return Err("`kind` is not text".into()),
    };
    let writes = match &raw["writes"] {
        Value::Null => None,
        Value::Bool(b) => Some(*b),
        _ => return Err("`writes` is not true or false".into()),
    };
    let steps = raw["steps"]
        .as_array()
        .ok_or("`steps` is not a list")?
        .iter()
        .map(parse_step)
        .collect::<Result<Vec<_>, _>>()?;
    Ok(Flow {
        name,
        site: text(&raw["site"]),
        description: text(&raw["description"]),
        start_url: text(&raw["startUrl"]),
        params: param_names(&raw["params"])?,
        kind,
        writes,
        verified: raw["verified"].as_bool(),
        steps,
        raw: raw.clone(),
    })
}

// ---- the hash ----

/// JSON with the keys of every object sorted, so the same content always gives the same text.
pub fn canonical(v: &Value) -> String {
    let mut out = String::new();
    write_canonical(v, &mut out);
    out
}
fn write_canonical(v: &Value, out: &mut String) {
    match v {
        Value::Object(map) => {
            let mut keys: Vec<&String> = map.keys().collect();
            keys.sort();
            out.push('{');
            for (i, key) in keys.into_iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                out.push_str(&Value::String(key.clone()).to_string());
                out.push(':');
                write_canonical(&map[key], out);
            }
            out.push('}');
        }
        Value::Array(items) => {
            out.push('[');
            for (i, item) in items.iter().enumerate() {
                if i > 0 {
                    out.push(',');
                }
                write_canonical(item, out);
            }
            out.push(']');
        }
        other => out.push_str(&other.to_string()),
    }
}

/// The content hash an approval and a grant are keyed by: sha256 of the canonical JSON of the
/// `flow show` `data.flow` (`createdAt` kept) and the parameters.
pub fn flow_hash(raw: &Value, params: &BTreeMap<String, String>) -> String {
    let body = canonical(&json!({"flow": raw, "params": params}));
    Sha256::digest(body.as_bytes())
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}

// ---- the addresses a flow names ----

/// Whether a text is an address (with a scheme) or a scheme that must never be opened. Text with
/// a space in it is a sentence, not an address.
fn looks_like_url(s: &str) -> bool {
    let lower = s.trim().to_lowercase();
    if lower.chars().any(char::is_whitespace) {
        return false;
    }
    let schemes = [
        "javascript:",
        "data:",
        "file:",
        "about:",
        "chrome:",
        "chrome-error:",
        "view-source:",
        "blob:",
        "vbscript:",
        "mailto:",
    ];
    schemes.iter().any(|p| lower.starts_with(p))
        || lower.split_once("://").is_some_and(|(scheme, _)| {
            !scheme.is_empty()
                && scheme
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '.' | '-'))
        })
}

/// Every address the flow names: first the places that must hold one (`startUrl`, `origin`,
/// `hosts`, each step's `url` and `open` value), whatever they contain, then every other text of
/// `data.flow` that looks like an address (a recorded request, a place nobody listed). `(address,
/// where it was found)`, each address once.
pub fn urls_of(flow: &Flow) -> Vec<(String, String)> {
    let mut found: Vec<(String, String)> = Vec::new();
    let mut add = |url: &str, label: String| {
        let url = url.trim();
        if !url.is_empty() && !found.iter().any(|(u, _)| u == url) {
            found.push((url.to_owned(), label));
        }
    };
    add(&flow.start_url, "startUrl".into());
    add(flow.raw["origin"].as_str().unwrap_or(""), "origin".into());
    for host in flow.raw["hosts"].as_array().into_iter().flatten() {
        add(host.as_str().unwrap_or(""), "hosts".into());
    }
    for (i, step) in flow.steps.iter().enumerate() {
        let label = step
            .seq
            .map_or_else(|| format!("step {}", i + 1), |n| format!("step {n}"));
        add(&step.url, label.clone());
        if let StepKind::Open(url) = &step.kind {
            add(url, label);
        }
    }
    fn walk(v: &Value, label: &str, add: &mut dyn FnMut(&str, String)) {
        match v {
            Value::String(s) if looks_like_url(s) => add(s, label.to_owned()),
            Value::Array(items) => {
                for (i, item) in items.iter().enumerate() {
                    let label = if label == "steps" {
                        item["seq"]
                            .as_i64()
                            .map_or_else(|| format!("step {}", i + 1), |n| format!("step {n}"))
                    } else {
                        label.to_owned()
                    };
                    walk(item, &label, add);
                }
            }
            Value::Object(map) => {
                for (key, value) in map {
                    if matches!(key.as_str(), "createdBy" | "description" | "intent") {
                        continue;
                    }
                    let label = if label.starts_with("step ") {
                        label.to_owned()
                    } else {
                        key.clone()
                    };
                    walk(value, &label, add);
                }
            }
            _ => {}
        }
    }
    walk(&flow.raw, "flow", &mut add);
    found
}

// ---- parameters in steps ----

fn placeholders(name: &str) -> [String; 6] {
    [
        format!("{{{{{name}}}}}"),
        format!("{{{name}}}"),
        format!("${{{name}}}"),
        format!("${name}"),
        format!("<{name}>"),
        format!("%{name}%"),
    ]
}

/// Whether a step names parameter `name` (as `{name}`, `{{name}}`, `${name}`, `$name`, `<name>`
/// or `%name%`) in its command or value. Not measured: how Ardia writes it.
pub fn uses_param(step: &Step, name: &str) -> bool {
    let pats = placeholders(name);
    step.command
        .iter()
        .map(String::as_str)
        .chain(step.value.as_deref())
        .any(|t| pats.iter().any(|p| t.contains(p.as_str())))
}

/// `text` with the placeholders of the given parameters replaced by their values.
pub fn substitute(text: &str, params: &BTreeMap<String, String>) -> String {
    let mut out = text.to_owned();
    for (key, value) in params {
        for pattern in placeholders(key) {
            out = out.replace(pattern.as_str(), value);
        }
    }
    out
}
