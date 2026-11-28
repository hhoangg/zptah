//! Shared builders for workflow tests, ported from the React tests and `workflowUiFixtures.ts`.
use super::model::{Edge, Node, Workflow};
use super::ordered::Json;
use super::parse::{parse_edge, parse_node, parse_workflow};

pub fn json(text: &str) -> Json {
    Json::parse(text).unwrap_or_else(|| panic!("bad fixture json: {text}"))
}

pub fn workflow_from(text: &str) -> Workflow {
    parse_workflow(&json(text)).unwrap_or_else(|| panic!("fixture is not a workflow: {text}"))
}

pub fn node(text: &str) -> Node {
    parse_node(&json(text)).unwrap_or_else(|| panic!("fixture is not a node: {text}"))
}

pub fn edge(text: &str) -> Edge {
    parse_edge(&json(text)).unwrap_or_else(|| panic!("fixture is not an edge: {text}"))
}

/// The ClickUp -> Dev -> Review example from the design spec (section 9.1), as stored.
pub fn example() -> Workflow {
    workflow_from(EXAMPLE_JSON)
}

/// The same example laid out on the board (`ticketWorkflow` in the React UI fixtures).
pub fn ticket_workflow() -> Workflow {
    workflow_from(TICKET_JSON)
}

/// `{ ...node, ...patch }` for the node `id`, like the React tests' `patchNode`. `patch` is a
/// JSON object text; the patched node is read back through the lenient parser.
pub fn patch_node(w: &Workflow, id: &str, patch: &str) -> Workflow {
    let mut out = w.clone();
    let slot = out
        .nodes
        .iter_mut()
        .find(|n| n.id == id)
        .expect("node to patch");
    let Json::Object(mut fields) = json(&serde_json::to_string(&*slot).unwrap()) else {
        unreachable!()
    };
    let Json::Object(patch) = json(patch) else {
        panic!("patch must be an object")
    };
    for (key, value) in patch.iter() {
        // A node is written with `outputs`; older fixtures patch it as `output`.
        let key = if key == "output" { "outputs" } else { key };
        fields.insert(key.to_string(), value.clone());
    }
    *slot = parse_node(&Json::Object(fields)).expect("patched node still parses");
    out
}

/// The example plus a command node run right after `fetch`.
pub fn with_command(command: &str) -> Workflow {
    let mut w = example();
    w.nodes.push(node(&format!(
        r#"{{"id":"lint","type":"command","project":"{{{{inputs.project}}}}","command":{}}}"#,
        serde_json::to_string(command).unwrap()
    )));
    w.edges.push(edge(r#"{"from":"fetch","to":"lint"}"#));
    w
}

/// A workflow that uses every node kind and field, with awkward characters.
pub const SINK_JSON: &str = r###"{
  "id": "wf-sink0001",
  "name": "Sink \"quoted\" \\ é 日本 \u0001",
  "description": "line1\nline2\ttab",
  "inputs": {
    "zeta": { "type": "string", "description": "last alphabetically, first declared" },
    "project": { "type": "project" },
    "count": { "type": "number" },
    "flag": { "type": "boolean" }
  },
  "inputValues": { "project": "/x", "count": 2.5, "flag": true },
  "nodes": [
    { "id": "start", "type": "schedule", "title": "Every 4h",
      "schedule": { "scheduleKind": "interval", "time": "09:00", "minute": 0, "dayOfWeek": 1, "intervalHours": 4, "intervalFrom": "2026-01-01" },
      "ui": { "x": -16.5, "y": 10 } },
    { "id": "fetch", "type": "session", "project": "{{inputs.project}}", "prompt": "Say \"hi\" é\n/api/users",
      "checkout": "root", "outputs": { "b": { "type": "list", "inReport": true, "description": "b first" }, "a": { "type": "string" } },
      "harness": "claude", "model": "sonnet", "effort": "high", "reuseSession": false, "timeoutMinutes": 30 },
    { "id": "build", "type": "command", "project": "{{inputs.project}}", "command": "npm test {{inputs.zeta}}",
      "cwd": { "worktreeOf": "fetch" }, "timeoutMinutes": 1.5 },
    { "id": "ask", "type": "switch", "decide": "ai", "question": "ok?", "cases": ["yes", "no"] },
    { "id": "gate", "type": "if", "condition": "{{fetch.a}} == \"yes\"" },
    { "id": "gate2", "type": "if", "decide": "ai", "question": "really?" },
    { "id": "each", "type": "forEach", "in": "{{fetch.b}}", "as": "row", "maxParallel": 2 },
    { "id": "inner", "type": "session", "parent": "each", "project": "{{inputs.project}}", "prompt": "p",
      "checkout": { "worktreeOf": "fetch" }, "nestUnder": "fetch", "outputs": {} },
    { "id": "report", "type": "command", "project": "{{inputs.project}}", "command": "echo {{fetch.a}}" }
  ],
  "edges": [
    { "from": "start", "to": "fetch" },
    { "from": "fetch", "to": "build" },
    { "from": "build", "to": "ask" },
    { "from": "ask", "to": "gate", "handle": "yes" },
    { "from": "gate", "to": "gate2", "handle": "true" },
    { "from": "gate", "to": "report", "handle": "false" },
    { "from": "gate2", "to": "each", "handle": "true" },
    { "from": "gate2", "to": "report", "handle": "false" },
    { "from": "each", "to": "report" },
    { "from": "build", "to": "fetch", "loop": { "maxRounds": 2, "decide": "{{build.stdout}} contains \"FAIL\"" } }
  ],
  "enabled": true,
  "createdAt": 1700000000000
}
"###;

// These pin the Rust `definition_fingerprint` output for the same JSON (regenerated when the
// definition gained fields); they no longer come from the React app.
pub const SINK_FINGERPRINT: &str = r###"{"description":"line1\nline2\ttab","edges":[{"from":"start","to":"fetch"},{"from":"fetch","to":"build"},{"from":"build","to":"ask"},{"from":"ask","handle":"yes","to":"gate"},{"from":"gate","handle":"true","to":"gate2"},{"from":"gate","handle":"false","to":"report"},{"from":"gate2","handle":"true","to":"each"},{"from":"gate2","handle":"false","to":"report"},{"from":"each","to":"report"},{"from":"build","loop":{"decide":"{{build.stdout}} contains \"FAIL\"","maxRounds":2},"to":"fetch"}],"inputs":{"count":{"type":"number"},"flag":{"type":"boolean"},"project":{"type":"project"},"zeta":{"description":"last alphabetically, first declared","type":"string"}},"name":"Sink \"quoted\" \\ é 日本 \u0001","nodes":[{"id":"start","schedule":{"dayOfWeek":1,"intervalFrom":"2026-01-01","intervalHours":4,"minute":0,"scheduleKind":"interval","time":"09:00"},"title":"Every 4h","type":"schedule"},{"checkout":"root","effort":"high","harness":"claude","id":"fetch","model":"sonnet","outputs":{"a":{"type":"string"},"b":{"description":"b first","inReport":true,"type":"list"}},"project":"{{inputs.project}}","prompt":"Say \"hi\" é\n/api/users","reuseSession":false,"timeoutMinutes":30,"type":"session"},{"command":"npm test {{inputs.zeta}}","cwd":{"worktreeOf":"fetch"},"id":"build","project":"{{inputs.project}}","timeoutMinutes":1.5,"type":"command"},{"cases":["yes","no"],"decide":"ai","id":"ask","question":"ok?","type":"switch"},{"condition":"{{fetch.a}} == \"yes\"","id":"gate","type":"if"},{"decide":"ai","id":"gate2","question":"really?","type":"if"},{"as":"row","id":"each","in":"{{fetch.b}}","maxParallel":2,"type":"forEach"},{"checkout":{"worktreeOf":"fetch"},"id":"inner","nestUnder":"fetch","outputs":{},"parent":"each","project":"{{inputs.project}}","prompt":"p","type":"session"},{"command":"echo {{fetch.a}}","id":"report","project":"{{inputs.project}}","type":"command"}]}"###;
pub const EXAMPLE_FINGERPRINT: &str = r###"{"description":"Pull my ClickUp tickets, implement each, review up to 3 rounds.","edges":[{"from":"start","to":"fetch"},{"from":"fetch","to":"each"},{"from":"dev","to":"review"},{"from":"each","to":"report"},{"from":"review","loop":{"decide":"ai","maxRounds":3},"to":"dev"}],"inputs":{"clickupList":{"description":"ClickUp list to pull from","type":"string"},"project":{"type":"project"}},"name":"ClickUp -> Dev -> Review","nodes":[{"id":"start","schedule":{"dayOfWeek":1,"minute":0,"scheduleKind":"weekdays","time":"09:00"},"type":"schedule"},{"checkout":"root","id":"fetch","model":"sonnet","outputs":{"tickets":{"type":"list"}},"project":"{{inputs.project}}","prompt":"List open tickets assigned to me in {{inputs.clickupList}}.","type":"session"},{"as":"item","id":"each","in":"{{fetch.tickets}}","maxParallel":3,"type":"forEach"},{"checkout":"newWorktree","id":"dev","model":"sonnet","outputs":{"pr_url":{"type":"string"},"summary":{"type":"string"}},"parent":"each","project":"{{inputs.project}}","prompt":"Implement {{item.title}} ({{item.url}}) and open a PR.\n{{#if review}}Fix these review issues: {{review.issues}}{{/if}}","type":"session"},{"checkout":{"worktreeOf":"dev"},"id":"review","model":"opus","nestUnder":"dev","outputs":{"issues":{"type":"list"},"verdict":{"type":"string"}},"parent":"each","project":"{{inputs.project}}","prompt":"Review {{dev.pr_url}}. List blocking issues only.","type":"session"},{"command":"echo done","id":"report","project":"{{inputs.project}}","type":"command"}]}"###;
pub const TICKET_FINGERPRINT: &str = r###"{"description":"Pull my ClickUp tickets, implement each, review up to 3 rounds.","edges":[{"from":"start","to":"fetch"},{"from":"fetch","to":"each"},{"from":"dev","to":"review"},{"from":"each","to":"report"},{"from":"review","loop":{"decide":"ai","maxRounds":3},"to":"dev"}],"inputs":{"clickupList":{"description":"ClickUp list to pull from","type":"string"},"project":{"type":"project"}},"name":"ClickUp -> Dev -> Review","nodes":[{"id":"start","schedule":{"dayOfWeek":1,"minute":0,"scheduleKind":"weekdays","time":"09:00"},"title":"Weekdays","type":"schedule"},{"checkout":"root","id":"fetch","model":"claude:sonnet-5","outputs":{"tickets":{"type":"list"}},"project":"{{inputs.project}}","prompt":"List open tickets assigned to me in {{inputs.clickupList}}.","title":"Fetch tickets","type":"session"},{"as":"item","id":"each","in":"{{fetch.tickets}}","maxParallel":3,"title":"Each ticket","type":"forEach"},{"checkout":"newWorktree","id":"dev","model":"claude:sonnet-5","outputs":{"pr_url":{"type":"string"},"summary":{"type":"string"}},"parent":"each","project":"{{inputs.project}}","prompt":"Implement {{item.title}} ({{item.url}}) and open a PR.\n{{#if review}}Fix these review issues: {{review.issues}}{{/if}}","title":"Dev","type":"session"},{"checkout":{"worktreeOf":"dev"},"id":"review","model":"claude:opus-5","nestUnder":"dev","outputs":{"issues":{"type":"list"},"verdict":{"type":"string"}},"parent":"each","project":"{{inputs.project}}","prompt":"Review {{dev.pr_url}}. List blocking issues only.","title":"Review","type":"session"},{"command":"echo done","id":"report","project":"{{inputs.project}}","title":"Report","type":"command"}]}"###;

pub const EXAMPLE_JSON: &str = r#"{
  "id": "wf-abcd1234",
  "name": "ClickUp -> Dev -> Review",
  "description": "Pull my ClickUp tickets, implement each, review up to 3 rounds.",
  "inputs": {
    "project": { "type": "project" },
    "clickupList": { "type": "string", "description": "ClickUp list to pull from" }
  },
  "inputValues": { "project": "/Users/x/repo", "clickupList": "Sprint" },
  "nodes": [
    { "id": "start", "type": "schedule",
      "schedule": { "scheduleKind": "weekdays", "time": "09:00", "minute": 0, "dayOfWeek": 1 },
      "ui": { "x": 0, "y": 0 } },
    { "id": "fetch", "type": "session", "project": "{{inputs.project}}", "model": "sonnet",
      "checkout": "root",
      "prompt": "List open tickets assigned to me in {{inputs.clickupList}}.",
      "outputs": { "tickets": { "type": "list" } } },
    { "id": "each", "type": "forEach", "in": "{{fetch.tickets}}", "as": "item", "maxParallel": 3 },
    { "id": "dev", "type": "session", "parent": "each", "project": "{{inputs.project}}",
      "model": "sonnet", "checkout": "newWorktree",
      "prompt": "Implement {{item.title}} ({{item.url}}) and open a PR.\n{{#if review}}Fix these review issues: {{review.issues}}{{/if}}",
      "outputs": { "pr_url": { "type": "string" }, "summary": { "type": "string" } } },
    { "id": "review", "type": "session", "parent": "each", "project": "{{inputs.project}}",
      "model": "opus", "checkout": { "worktreeOf": "dev" }, "nestUnder": "dev",
      "prompt": "Review {{dev.pr_url}}. List blocking issues only.",
      "outputs": { "verdict": { "type": "string" }, "issues": { "type": "list" } } },
    { "id": "report", "type": "command", "project": "{{inputs.project}}", "command": "echo done" }
  ],
  "edges": [
    { "from": "start", "to": "fetch" },
    { "from": "fetch", "to": "each" },
    { "from": "dev", "to": "review" },
    { "from": "each", "to": "report" },
    { "from": "review", "to": "dev", "loop": { "maxRounds": 3, "decide": "ai" } }
  ],
  "enabled": true,
  "createdAt": 1700000000000
}"#;

pub const TICKET_JSON: &str = r#"{
  "id": "wf-ticket001",
  "name": "ClickUp -> Dev -> Review",
  "description": "Pull my ClickUp tickets, implement each, review up to 3 rounds.",
  "inputs": {
    "project": { "type": "project" },
    "clickupList": { "type": "string", "description": "ClickUp list to pull from" }
  },
  "inputValues": { "project": "/work/api", "clickupList": "Sprint" },
  "nodes": [
    { "id": "start", "type": "schedule", "title": "Weekdays",
      "schedule": { "scheduleKind": "weekdays", "time": "09:00", "minute": 0, "dayOfWeek": 1 },
      "ui": { "x": 16, "y": 50 } },
    { "id": "fetch", "type": "session", "title": "Fetch tickets", "project": "{{inputs.project}}",
      "model": "claude:sonnet-5", "checkout": "root",
      "prompt": "List open tickets assigned to me in {{inputs.clickupList}}.",
      "outputs": { "tickets": { "type": "list" } }, "ui": { "x": 336, "y": 42 } },
    { "id": "each", "type": "forEach", "title": "Each ticket", "in": "{{fetch.tickets}}",
      "as": "item", "maxParallel": 3, "ui": { "x": 640, "y": 0 } },
    { "id": "dev", "type": "session", "title": "Dev", "parent": "each",
      "project": "{{inputs.project}}", "model": "claude:sonnet-5", "checkout": "newWorktree",
      "prompt": "Implement {{item.title}} ({{item.url}}) and open a PR.\n{{#if review}}Fix these review issues: {{review.issues}}{{/if}}",
      "outputs": { "pr_url": { "type": "string" }, "summary": { "type": "string" } },
      "ui": { "x": 16, "y": 32 } },
    { "id": "review", "type": "session", "title": "Review", "parent": "each",
      "project": "{{inputs.project}}", "model": "claude:opus-5",
      "checkout": { "worktreeOf": "dev" }, "nestUnder": "dev",
      "prompt": "Review {{dev.pr_url}}. List blocking issues only.",
      "outputs": { "verdict": { "type": "string" }, "issues": { "type": "list" } },
      "ui": { "x": 320, "y": 32 } },
    { "id": "report", "type": "command", "title": "Report", "project": "{{inputs.project}}",
      "command": "echo done", "ui": { "x": 1324, "y": 42 } }
  ],
  "edges": [
    { "from": "start", "to": "fetch" },
    { "from": "fetch", "to": "each" },
    { "from": "dev", "to": "review" },
    { "from": "each", "to": "report" },
    { "from": "review", "to": "dev", "loop": { "maxRounds": 3, "decide": "ai" } }
  ],
  "ui": { "direction": "lr" },
  "enabled": true,
  "createdAt": 1
}"#;

/// A machine whose environment holds a secret: `TOKEN` is set in the process, `LOGIN_TOKEN` only
/// in the login shell (whose raw output is noisy with the value), `MISSING` nowhere. The value is
/// what a leak would carry; a preflight built on this probe must never repeat it.
pub struct SecretProbe(pub &'static str);

impl super::preflight::PreflightProbe for SecretProbe {
    fn cli_found(&self, _: &str) -> bool {
        false
    }
    fn env_process(&self, name: &str) -> bool {
        name == "TOKEN"
    }
    fn env_login(&self, names: &[String]) -> Option<Vec<String>> {
        // What a login shell would print, secret included, filtered the way the real probe does.
        let raw = format!("{}\nLOGIN_TOKEN\n", self.0);
        Some(super::preflight::names_in_output(&raw, names))
    }
    fn mcp_servers(&self, _: &[String]) -> Result<Vec<crate::mcp::McpConnection>, String> {
        Err(format!("cannot list ({})", self.0))
    }
    fn mcp_statuses(&self, _: &str) -> Option<Vec<(String, String)>> {
        None
    }
}
