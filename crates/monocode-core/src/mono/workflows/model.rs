//! The workflow definition and run state: a graph of nodes the app runs for the user.
//!
//! Field names and shapes match the React app's `workflow.ts`, so `workflows.json` and the run
//! files are shared between both apps. Definitions never hold absolute paths or session ids.
use super::event::EventTrigger;
use super::ordered::{serialize_number, serialize_opt_number, write_number, Json, OrderedMap};
use super::parse;
use crate::mono::habits::Schedule;
use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use std::collections::BTreeMap;

pub type WorkflowId = String;
/// Matches `is_node_id`, so it is safe inside a `{{nodeId.field}}` path.
pub type NodeId = String;
/// "" for top level, `<forEachId>#<index>` inside a group, archived `<forEachId>#<index>@r<round>`.
pub type BranchKey = String;

/// The most workflows the global store holds.
pub const WORKFLOWS_MAX: usize = 100;
/// The longest time limit a session or command node may have: 24 hours.
pub const MAX_TIMEOUT_MINUTES: u32 = 1440;
pub const WORKFLOW_RUNS_KEPT: usize = 20;
/// The most iterations of a For each that run at once.
pub const MAX_PARALLEL_LIMIT: u32 = 10;
/// How many run at once when `maxParallel` is unset.
pub const DEFAULT_MAX_PARALLEL: u32 = 3;
/// The most runs of one workflow that go at the same time (`concurrency.max`).
pub const CONCURRENCY_LIMIT: u32 = 10;
/// How many runs of one workflow go at once when `concurrency` is unset.
pub const DEFAULT_CONCURRENCY: u32 = 3;
/// The most runs of one workflow that may wait in the queue.
pub const QUEUED_RUNS_MAX: usize = 50;
/// The branch a run worktree starts from when `workspace.baseBranch` is unset.
pub const DEFAULT_BASE_BRANCH: &str = "main";
/// The most passes a loop edge may allow (`maxRounds`).
pub const MAX_ROUNDS_LIMIT: u32 = 10;
pub const DEFAULT_SESSION_TIMEOUT_MINUTES: u32 = 120;
pub const DEFAULT_COMMAND_TIMEOUT_MINUTES: u32 = 10;
/// Limits on a definition sent through the app CLI.
pub const WORKFLOW_NAME_MAX: usize = 80;
pub const WORKFLOW_DESCRIPTION_MAX: usize = 1_000;
pub const WORKFLOW_DEFINITION_MAX_BYTES: usize = 512 * 1024;

pub const NODE_TYPES: [&str; 8] = [
    "schedule", "manual", "event", "session", "command", "if", "switch", "forEach",
];

/// The access modes a session node may name (`access`). `read-only` is not offered.
pub const SESSION_ACCESS_IDS: [&str; 4] =
    ["supervised", "auto-accept-edits", "auto", "full-access"];
/// The access a session node uses when it names none.
pub const DEFAULT_SESSION_ACCESS: &str = "auto-accept-edits";
/// The access that lets an agent run anything without asking.
pub const FULL_ACCESS: &str = "full-access";

/// The most trigger nodes a workflow may have. Any of them can start a run.
pub const TRIGGERS_MAX: usize = 5;

/// The handle a switch sends the run down when no case matches. Not allowed as a case.
pub const SWITCH_DEFAULT: &str = "default";

/// `^[a-z][a-z0-9_]{0,39}$`
pub fn is_node_id(id: &str) -> bool {
    let b = id.as_bytes();
    (1..=40).contains(&b.len())
        && b[0].is_ascii_lowercase()
        && b[1..]
            .iter()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == b'_')
}

const BASE36: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// "wf-" followed by 8 lowercase base36 characters.
pub fn new_workflow_id() -> WorkflowId {
    let bytes: [u8; 8] = rand::random();
    // 256 is not a multiple of 36, so the modulo skews slightly; an id is a label, not a secret.
    let id: String = bytes
        .iter()
        .map(|b| BASE36[(*b % 36) as usize] as char)
        .collect();
    format!("wf-{id}")
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum OutputFieldType {
    String,
    Number,
    Boolean,
    List,
    Object,
}

impl OutputFieldType {
    pub const ALL: [(&str, Self); 5] = [
        ("string", Self::String),
        ("number", Self::Number),
        ("boolean", Self::Boolean),
        ("list", Self::List),
        ("object", Self::Object),
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct OutputField {
    #[serde(rename = "type")]
    pub ty: OutputFieldType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub in_report: Option<bool>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl OutputField {
    pub fn new(ty: OutputFieldType) -> Self {
        Self {
            ty,
            in_report: None,
            description: None,
        }
    }
}

/// How a session node ends.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionMode {
    /// The node finishes when the first turn ends.
    #[default]
    Auto,
    /// The first turn ends, then the user chats on and continues or fails the step.
    WaitForMe,
}

impl SessionMode {
    pub const ALL: [(&str, Self); 2] = [("auto", Self::Auto), ("waitForMe", Self::WaitForMe)];
    /// Absent means `auto`; None for a value that is not a mode.
    pub fn parse(text: Option<&str>) -> Option<Self> {
        match text {
            None => Some(Self::Auto),
            Some(text) => Self::ALL
                .iter()
                .find(|(name, _)| *name == text)
                .map(|(_, mode)| *mode),
        }
    }
}

/// How a session node is shown.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum SessionSurface {
    #[default]
    Chat,
    /// The provider's own CLI in a terminal pane (core `Surface::Tui`).
    Terminal,
}

impl SessionSurface {
    pub const ALL: [(&str, Self); 2] = [("chat", Self::Chat), ("terminal", Self::Terminal)];
    /// Absent means `chat`; None for a value that is not a surface.
    pub fn parse(text: Option<&str>) -> Option<Self> {
        match text {
            None => Some(Self::Chat),
            Some(text) => Self::ALL
                .iter()
                .find(|(name, _)| *name == text)
                .map(|(_, surface)| *surface),
        }
    }
    pub fn is_terminal(self) -> bool {
        self == Self::Terminal
    }
    /// The surface the app opens the session in.
    pub fn core(self) -> crate::surface::Surface {
        match self {
            Self::Chat => crate::surface::Surface::Chat,
            Self::Terminal => crate::surface::Surface::Tui,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Checkout {
    /// The run's own worktree (the project folder when the workspace makes none). The default.
    Run,
    Root,
    NewWorktree,
    WorktreeOf(NodeId),
    /// A folder the user names, which must be a worktree of the step's project.
    Existing(String),
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum Cwd {
    Root,
    WorktreeOf(NodeId),
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "lowercase")]
pub enum InputType {
    Project,
    String,
    Number,
    Boolean,
}

impl InputType {
    pub const ALL: [(&str, Self); 4] = [
        ("project", Self::Project),
        ("string", Self::String),
        ("number", Self::Number),
        ("boolean", Self::Boolean),
    ];
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct WorkflowInput {
    #[serde(rename = "type")]
    pub ty: InputType,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
}

impl WorkflowInput {
    pub fn new(ty: InputType) -> Self {
        Self {
            ty,
            description: None,
        }
    }
}

/// The value of a declared input.
#[derive(Clone, Debug, PartialEq)]
pub enum InputValue {
    String(String),
    Number(f64),
    Bool(bool),
}

impl Serialize for InputValue {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        match self {
            Self::String(v) => s.serialize_str(v),
            Self::Number(v) => write_number(*v, s),
            Self::Bool(v) => s.serialize_bool(*v),
        }
    }
}

/// What a workflow needs on the machine that runs it.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum RequirementKind {
    Cli,
    Env,
    Mcp,
}

impl RequirementKind {
    pub const ALL: [Self; 3] = [Self::Cli, Self::Env, Self::Mcp];

    /// The key a requirement is written under in the file.
    pub fn key(self) -> &'static str {
        match self {
            Self::Cli => "cli",
            Self::Env => "env",
            Self::Mcp => "mcp",
        }
    }
}

/// One declared precondition: a command on PATH, an environment variable, or an MCP server.
/// It is only ever looked up by name, never run. The hint tells the user how to satisfy it.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Requirement {
    pub kind: RequirementKind,
    pub name: String,
    pub hint: Option<String>,
}

impl Serialize for Requirement {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1 + usize::from(self.hint.is_some())))?;
        map.serialize_entry(self.kind.key(), &self.name)?;
        if let Some(hint) = &self.hint {
            map.serialize_entry("hint", hint)?;
        }
        map.end()
    }
}

/// Where a run works.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum WorkspaceMode {
    /// The run makes one worktree of `workspace.project` and its steps work in it.
    #[default]
    PerRun,
    /// The run makes no worktree: steps work where their own checkout says.
    None,
}

impl WorkspaceMode {
    pub const ALL: [(&str, Self); 2] = [("perRun", Self::PerRun), ("none", Self::None)];
    pub fn name(self) -> &'static str {
        match self {
            Self::PerRun => "perRun",
            Self::None => "none",
        }
    }
}

/// `workspace` of a workflow: whether a run gets a worktree, and from which project and branch.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
pub struct Workspace {
    pub mode: WorkspaceMode,
    /// A project path or `{{inputs.<name>}}`. Empty when the workflow names none.
    pub project: String,
    /// The branch the run worktree starts from; `main` when unset.
    pub base_branch: Option<String>,
}

impl Workspace {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// The branch a run worktree starts from.
    pub fn base(&self) -> &str {
        self.base_branch
            .as_deref()
            .map(str::trim)
            .filter(|b| !b.is_empty())
            .unwrap_or(DEFAULT_BASE_BRANCH)
    }
    /// Whether a run makes a worktree of its own.
    pub fn per_run(&self) -> bool {
        self.mode == WorkspaceMode::PerRun && !self.project.trim().is_empty()
    }
}

impl Serialize for Workspace {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("mode", self.mode.name())?;
        if !self.project.is_empty() {
            map.serialize_entry("project", &self.project)?;
        }
        if let Some(base) = &self.base_branch {
            map.serialize_entry("baseBranch", base)?;
        }
        map.end()
    }
}

/// `concurrency` of a workflow: how many of its runs go at the same time.
#[derive(Clone, Copy, Debug, PartialEq)]
pub struct Concurrency {
    /// 1 to 10 (as written, so validation can name a bad value).
    pub max: f64,
}

impl Default for Concurrency {
    fn default() -> Self {
        Self {
            max: f64::from(DEFAULT_CONCURRENCY),
        }
    }
}

impl Concurrency {
    pub fn is_default(&self) -> bool {
        *self == Self::default()
    }
    /// The number of live runs allowed, whatever was written.
    pub fn limit(&self) -> usize {
        if !self.max.is_finite() {
            return DEFAULT_CONCURRENCY as usize;
        }
        (self.max.round() as i64).clamp(1, i64::from(CONCURRENCY_LIMIT)) as usize
    }
}

impl Serialize for Concurrency {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(Some(1))?;
        map.serialize_entry("max", &SerNumber(self.max))?;
        map.end()
    }
}

struct SerNumber(f64);
impl Serialize for SerNumber {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        serialize_number(&self.0, s)
    }
}

/// Board position, kept so an exported file opens with the same layout.
#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
pub struct Ui {
    #[serde(serialize_with = "serialize_number")]
    pub x: f64,
    #[serde(serialize_with = "serialize_number")]
    pub y: f64,
}

/// Workflow-level board settings, kept so a file opens laid out the same way. `direction` is
/// "lr" once the board has laid the workflow out left to right; a file without it holds
/// top-to-bottom positions and is laid out once on open.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
pub struct WorkflowUi {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub direction: Option<String>,
}

/// The `ui.direction` value of a workflow laid out left to right.
pub const DIRECTION_LR: &str = "lr";

impl WorkflowUi {
    /// The marker of a left-to-right workflow.
    pub fn lr() -> Self {
        Self {
            direction: Some(DIRECTION_LR.into()),
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub enum Decide {
    /// A one-shot AI call with no tools. Files written before this name read `"mono"` as `"ai"`.
    #[serde(rename = "ai")]
    Ai,
}

/// The `decide` value of a loop edge that asks the AI (the same word as `Decide::Ai`).
pub const DECIDE_AI: &str = "ai";

/// A model to run a one-shot call with: the harness, its model id and an optional effort. A
/// missing `model` means that harness's default model, resolved when the run starts (like a
/// session step that names only a harness).
#[derive(Clone, Debug, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DecisionModel {
    pub harness: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub model: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub effort: Option<String>,
}

/// Who gets the run summary besides the run page.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ReportTo {
    /// The Mono whose chat gets the summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub mono: Option<String>,
}

/// What started a run.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CallerKind {
    Manual,
    Schedule,
    Event,
    Mono,
}

impl CallerKind {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Manual => "manual",
            Self::Schedule => "schedule",
            Self::Event => "event",
            Self::Mono => "mono",
        }
    }
}

/// One run asked for while another request was still waiting to be claimed.
#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct RunRequest {
    #[serde(skip_serializing_if = "Option::is_none")]
    pub caller: Option<RunCaller>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub values: Option<OrderedMap<InputValue>>,
}

impl Workflow {
    /// Asks for one more run: it becomes the pending request, or waits behind the ones already
    /// asked for, so a request is never overwritten.
    pub fn push_run_request(&mut self, request: RunRequest) {
        if self.run_requested {
            self.run_backlog.push(request);
        } else {
            self.run_requested = true;
            self.run_caller = request.caller;
            self.run_values = request.values;
        }
    }

    /// How many requested runs no claim has answered yet.
    pub fn pending_requests(&self) -> usize {
        usize::from(self.run_requested) + self.run_backlog.len()
    }

    /// How many requested runs were asked for by `mono` (the user's have no Mono).
    pub fn pending_requests_of(&self, mono: &str) -> usize {
        let own = |caller: &Option<RunCaller>| {
            caller.as_ref().and_then(|c| c.mono_id.as_deref()) == Some(mono)
        };
        usize::from(self.run_requested && own(&self.run_caller))
            + self.run_backlog.iter().filter(|r| own(&r.caller)).count()
    }

    /// Drops the requests `mono` asked for, and no one else's; the next one left becomes the
    /// pending request. Returns how many were dropped.
    pub fn cancel_requests_of(&mut self, mono: &str) -> usize {
        let own = |caller: &Option<RunCaller>| {
            caller.as_ref().and_then(|c| c.mono_id.as_deref()) == Some(mono)
        };
        let mut all: Vec<RunRequest> = Vec::new();
        if self.run_requested {
            all.push(RunRequest {
                caller: self.run_caller.take(),
                values: self.run_values.take(),
            });
        }
        all.append(&mut self.run_backlog);
        let before = all.len();
        all.retain(|r| !own(&r.caller));
        let dropped = before - all.len();
        self.run_requested = false;
        for request in all {
            self.push_run_request(request);
        }
        dropped
    }

    /// Drops every request that was not claimed. Returns how many there were.
    pub fn cancel_requests(&mut self) -> usize {
        let n = self.pending_requests();
        self.run_requested = false;
        self.run_caller = None;
        self.run_values = None;
        self.run_backlog.clear();
        n
    }
}

/// Who or what started a run, and through which trigger node. It replaces the old `RunTrigger`.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunCaller {
    pub kind: CallerKind,
    /// The Mono that started the run, when `kind` is `mono`.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mono_id: Option<String>,
    /// The trigger node that fired.
    pub trigger_node: NodeId,
    /// The item an event run is about (`source`, `kind`, `ref`, `title`, `url`, `repo`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub item: Option<Json>,
}

impl RunCaller {
    pub fn new(kind: CallerKind, trigger_node: impl Into<NodeId>) -> Self {
        Self {
            kind,
            mono_id: None,
            trigger_node: trigger_node.into(),
            item: None,
        }
    }

    pub fn mono(mono_id: impl Into<String>, trigger_node: impl Into<NodeId>) -> Self {
        Self {
            mono_id: Some(mono_id.into()),
            ..Self::new(CallerKind::Mono, trigger_node)
        }
    }
}

/// Where a session node runs when it is not on this computer.
#[derive(Clone, Debug, PartialEq)]
pub struct SessionTarget {
    /// As written: "codespace" or "sandbox"; anything else is kept so validation can report it.
    pub kind: String,
    pub instance: TargetInstance,
    /// Every other key of the target object, in written order, passed verbatim to
    /// `sessions.start`'s target (branch, machine, cpus, memory, ttl, template, image, kit).
    pub options: OrderedMap<Json>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum TargetInstance {
    /// Absent or "new": a new instance for this node.
    New,
    /// `{"sameAs": "<session node id>"}`: the instance the session of that node ran on.
    SameAs(NodeId),
    /// Any other value, kept verbatim so validation reports it and a round trip keeps it.
    Invalid(Json),
}

/// The option keys of a target whose value may hold `{{...}}` templates: the string valued ones.
/// `kind` and `instance` always stay literal (they decide whether an instance is made at all),
/// and so do `cpus` (a number) and `kit` (a fixed choice).
pub const TEMPLATABLE_TARGET_OPTIONS: [&str; 6] =
    ["branch", "machine", "memory", "ttl", "template", "image"];

impl SessionTarget {
    /// A short line for the board and the review: "Codespace (new)", "Sandbox (same as build)".
    /// A kind this app does not know is shown as written, and a bad instance as "invalid".
    pub fn summary(&self) -> String {
        let kind = match self.kind.as_str() {
            "codespace" => "Codespace",
            "sandbox" => "Sandbox",
            other => other,
        };
        match &self.instance {
            TargetInstance::New => format!("{kind} (new)"),
            TargetInstance::SameAs(id) => format!("{kind} (same as {id})"),
            TargetInstance::Invalid(_) => format!("{kind} (invalid instance)"),
        }
    }
}

impl Serialize for SessionTarget {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        use serde::ser::SerializeMap;
        let mut map = s.serialize_map(None)?;
        map.serialize_entry("kind", &self.kind)?;
        match &self.instance {
            TargetInstance::New => {}
            TargetInstance::SameAs(id) => {
                map.serialize_entry("instance", &serde_json::json!({ "sameAs": id }))?
            }
            TargetInstance::Invalid(raw) => map.serialize_entry("instance", raw)?,
        }
        for (key, value) in self.options.iter() {
            map.serialize_entry(key, value)?;
        }
        map.end()
    }
}

/// What a node is. The `type` tag and the fields sit next to the base fields in the JSON.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "camelCase")]
// A session carries every optional field, so it is much larger than the other kinds. Nodes are
// few and always held in a Vec, and boxing a field would change the public shape.
#[allow(clippy::large_enum_variant)]
pub enum NodeKind {
    Schedule {
        schedule: Schedule,
    },
    Manual,
    /// Starts a run for an Inbox item that matches the filter.
    Event(EventTrigger),
    #[serde(rename_all = "camelCase")]
    Session {
        /// A project path or `{{inputs.<name>}}`. Empty means `workspace.project`.
        #[serde(skip_serializing_if = "String::is_empty")]
        project: String,
        prompt: String,
        checkout: Checkout,
        /// The fields the node declares (`outputs` in the file; `output` is read too). They
        /// reach the run through `workflow.output` or the app's fallback extraction.
        #[serde(rename = "outputs")]
        output: OrderedMap<OutputField>,
        /// `auto` (default: the node ends when the turn ends) or `waitForMe`. Kept as written
        /// so validation can name a value it does not know; see `SessionMode`.
        #[serde(skip_serializing_if = "Option::is_none")]
        mode: Option<String>,
        /// `chat` (default) or `terminal`; see `SessionSurface`.
        #[serde(skip_serializing_if = "Option::is_none")]
        surface: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        harness: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        model: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        effort: Option<String>,
        /// Keep one session across loop rounds. Default true.
        #[serde(skip_serializing_if = "Option::is_none")]
        reuse_session: Option<bool>,
        #[serde(skip_serializing_if = "Option::is_none")]
        nest_under: Option<NodeId>,
        /// Default 120.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        timeout_minutes: Option<f64>,
        /// Run in a cloud instance instead of on this computer.
        #[serde(skip_serializing_if = "Option::is_none")]
        target: Option<SessionTarget>,
        /// The app Claude account (`"default"`, an id or a label) a NEW cloud instance runs
        /// with, as in `sessions.start`, where it sits next to `target`. Only a cloud step
        /// with a new instance takes one (validation refuses the rest).
        #[serde(skip_serializing_if = "Option::is_none")]
        account: Option<String>,
        /// `supervised`, `auto-accept-edits` (default), `auto` or `full-access`. Kept as written
        /// so validation can name a value it does not know; see `NodeKind::session_access`.
        #[serde(skip_serializing_if = "Option::is_none")]
        access: Option<String>,
        /// A session folder id the session is filed in; it overrides the workflow's own folder.
        /// Local sessions only. A deleted folder falls back to the workflow's folder.
        #[serde(skip_serializing_if = "Option::is_none")]
        folder: Option<String>,
        /// Link the session to the work item of the trigger item (local only, no remote write).
        /// Needs an event trigger upstream.
        #[serde(skip_serializing_if = "Option::is_none")]
        link_work_item: Option<bool>,
    },
    #[serde(rename_all = "camelCase")]
    Command {
        /// A project path or `{{inputs.<name>}}`. Empty means `workspace.project`.
        #[serde(skip_serializing_if = "String::is_empty")]
        project: String,
        command: String,
        #[serde(skip_serializing_if = "Option::is_none")]
        cwd: Option<Cwd>,
        /// `fail` (default) or `continue`: with `continue` a non-zero exit finishes the node with
        /// `exitCode`, `stdout` and `stderr`. Kept as written so validation can name a bad value.
        #[serde(skip_serializing_if = "Option::is_none")]
        on_failure: Option<String>,
        /// Default 10.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        timeout_minutes: Option<f64>,
    },
    /// Either `condition` or `decide: "ai"` (with an optional `question`).
    If {
        #[serde(skip_serializing_if = "Option::is_none")]
        condition: Option<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decide: Option<Decide>,
        #[serde(skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    /// Routes to the out edge whose handle equals `value` after rendering, or to `default`.
    /// Either `value` or `decide: "ai"` (with an optional `question`).
    Switch {
        #[serde(skip_serializing_if = "Option::is_none")]
        value: Option<String>,
        cases: Vec<String>,
        #[serde(skip_serializing_if = "Option::is_none")]
        decide: Option<Decide>,
        #[serde(skip_serializing_if = "Option::is_none")]
        question: Option<String>,
    },
    #[serde(rename_all = "camelCase")]
    ForEach {
        /// A list expression such as `{{fetch.tickets}}`.
        #[serde(rename = "in")]
        list: String,
        /// Name of the loop variable. Default "item".
        #[serde(rename = "as", skip_serializing_if = "Option::is_none")]
        item: Option<String>,
        /// 1 to 10. Default 3.
        #[serde(
            skip_serializing_if = "Option::is_none",
            serialize_with = "serialize_opt_number"
        )]
        max_parallel: Option<f64>,
    },
}

impl NodeKind {
    /// A session node's mode; `auto` for anything else, and for a value validation reports.
    pub fn session_mode(&self) -> SessionMode {
        match self {
            Self::Session { mode, .. } => SessionMode::parse(mode.as_deref()).unwrap_or_default(),
            _ => SessionMode::Auto,
        }
    }
    /// A session node's surface; `chat` for anything else, and for a value validation reports.
    pub fn session_surface(&self) -> SessionSurface {
        match self {
            Self::Session { surface, .. } => {
                SessionSurface::parse(surface.as_deref()).unwrap_or_default()
            }
            _ => SessionSurface::Chat,
        }
    }
    /// A session node's access id as it runs: the default when unset or not known.
    pub fn session_access(&self) -> &str {
        match self {
            Self::Session {
                access: Some(access),
                ..
            } if SESSION_ACCESS_IDS.contains(&access.as_str()) => access,
            _ => DEFAULT_SESSION_ACCESS,
        }
    }
    /// Whether a session node links the trigger item's work item.
    pub fn links_work_item(&self) -> bool {
        matches!(
            self,
            Self::Session {
                link_work_item: Some(true),
                ..
            }
        )
    }
    /// The `type` tag as written in the file.
    pub fn type_name(&self) -> &'static str {
        match self {
            Self::Schedule { .. } => "schedule",
            Self::Manual => "manual",
            Self::Event(_) => "event",
            Self::Session { .. } => "session",
            Self::Command { .. } => "command",
            Self::If { .. } => "if",
            Self::Switch { .. } => "switch",
            Self::ForEach { .. } => "forEach",
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Node {
    pub id: NodeId,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    /// The For each node this node runs inside.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub parent: Option<NodeId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<Ui>,
    #[serde(flatten)]
    pub kind: NodeKind,
}

impl Node {
    pub fn new(id: impl Into<NodeId>, kind: NodeKind) -> Self {
        Self {
            id: id.into(),
            title: None,
            parent: None,
            ui: None,
            kind,
        }
    }
    pub fn is_trigger(&self) -> bool {
        matches!(
            self.kind,
            NodeKind::Schedule { .. } | NodeKind::Manual | NodeKind::Event(_)
        )
    }
}

/// The handle an out edge of an If or switch node leaves from. It is written as a plain string:
/// "true" and "false" for an If, a case or "default" for a switch. Build one with
/// `Handle::named` so "true" and "false" are always the first two variants.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum Handle {
    True,
    False,
    Case(String),
}

impl Handle {
    pub fn named(name: &str) -> Self {
        match name {
            "true" => Self::True,
            "false" => Self::False,
            other => Self::Case(other.to_string()),
        }
    }

    pub fn as_str(&self) -> &str {
        match self {
            Self::True => "true",
            Self::False => "false",
            Self::Case(name) => name,
        }
    }
}

impl Serialize for Handle {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(self.as_str())
    }
}

/// A back edge: after the source node, the decider may send the branch back to `to`.
#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct LoopSpec {
    /// 1 to 10.
    #[serde(serialize_with = "serialize_number")]
    pub max_rounds: f64,
    /// "ai", or a condition such as `{{review.verdict}} != "approve"`.
    pub decide: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct Edge {
    pub from: NodeId,
    pub to: NodeId,
    /// Only on the out edges of an If or switch node.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub handle: Option<Handle>,
    #[serde(rename = "loop", skip_serializing_if = "Option::is_none")]
    pub loop_spec: Option<LoopSpec>,
}

impl Edge {
    pub fn new(from: impl Into<NodeId>, to: impl Into<NodeId>) -> Self {
        Self {
            from: from.into(),
            to: to.into(),
            handle: None,
            loop_spec: None,
        }
    }
}

fn is_false(v: &bool) -> bool {
    !*v
}

#[derive(Clone, Debug, PartialEq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Workflow {
    pub id: WorkflowId,
    pub name: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    pub inputs: OrderedMap<WorkflowInput>,
    /// Values for the declared inputs. Not exported.
    pub input_values: OrderedMap<InputValue>,
    /// What the workflow needs on this machine. Part of the shared definition.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub requires: Vec<Requirement>,
    /// Values for one run, laid over `input_values` by `with_run_values`. Never exported.
    #[serde(rename = "runValues", skip_serializing_if = "Option::is_none")]
    pub run_values: Option<OrderedMap<InputValue>>,
    pub nodes: Vec<Node>,
    pub edges: Vec<Edge>,
    /// Board settings (the layout direction). Not part of the definition: it never changes
    /// `definition_fingerprint`.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ui: Option<WorkflowUi>,
    pub enabled: bool,
    pub created_at: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub next_run_at: Option<u64>,
    /// The next time of each schedule trigger, by node id. Only used when a workflow has several
    /// schedule triggers; `next_run_at` then holds the earliest of them.
    #[serde(skip_serializing_if = "BTreeMap::is_empty")]
    pub next_run_at_by_node: BTreeMap<NodeId, u64>,
    #[serde(skip_serializing_if = "is_false")]
    pub run_requested: bool,
    /// Requests made from another window, kept in the file because only the main window drives
    /// runs; the driver clears each once it has acted on it.
    #[serde(skip_serializing_if = "is_false")]
    pub stop_requested: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub resume_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_run_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub imported_from: Option<String>,
    /// Set on a workflow made from a file (by the Mono through the CLI, or imported in the UI),
    /// on one a Mono adds, and on one a Mono redefines (its steps, prompts, commands, projects,
    /// models or report settings change). Only the user, in the panel, may enable or run it, and
    /// only an approval of this exact version clears the mark. Never exported.
    #[serde(skip_serializing_if = "is_false")]
    pub needs_review: bool,
    /// The model an If, Switch or loop edge that decides with AI asks. Without it the harness and
    /// model of the nearest upstream session node are used (see `decision_model_for`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub decision_model: Option<DecisionModel>,
    /// Who else gets the run summary.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub report_to: Option<ReportTo>,
    /// Shapes the run summary. Without it the app writes the default one.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub summary_template: Option<String>,
    /// Who asked for the pending `run_requested` run. App-managed, never exported.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub run_caller: Option<RunCaller>,
    /// Runs asked for behind the pending `run_requested` one (a Mono or habit that calls
    /// `workflows.run` several times in one turn), oldest first. Each is promoted to the pending
    /// request when the one before it was claimed, so none is dropped. App-managed, never exported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub run_backlog: Vec<RunRequest>,
    /// Whether a run gets a worktree, and from which project and branch.
    #[serde(skip_serializing_if = "Workspace::is_default")]
    pub workspace: Workspace,
    /// How many runs go at the same time.
    #[serde(skip_serializing_if = "Concurrency::is_default")]
    pub concurrency: Concurrency,
    /// Runs asked to stop by id (`workflows.stop` with `runId`). App-managed, never exported.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub stop_run_ids: Vec<String>,
    /// How many runs were numbered so far; the next run is this plus one. App-managed.
    #[serde(skip_serializing_if = "is_zero")]
    pub run_counter: u64,
}

fn is_zero(n: &u64) -> bool {
    *n == 0
}

impl Workflow {
    /// A disabled, empty workflow.
    pub fn new(id: impl Into<WorkflowId>, name: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            name: name.into(),
            description: None,
            inputs: OrderedMap::new(),
            input_values: OrderedMap::new(),
            requires: Vec::new(),
            run_values: None,
            nodes: Vec::new(),
            edges: Vec::new(),
            ui: None,
            enabled: false,
            created_at: 0,
            next_run_at: None,
            next_run_at_by_node: BTreeMap::new(),
            run_requested: false,
            stop_requested: false,
            resume_run_id: None,
            last_run_id: None,
            imported_from: None,
            needs_review: false,
            decision_model: None,
            report_to: None,
            summary_template: None,
            run_caller: None,
            run_backlog: Vec::new(),
            workspace: Workspace::default(),
            concurrency: Concurrency::default(),
            stop_run_ids: Vec::new(),
            run_counter: 0,
        }
    }
    pub fn node(&self, id: &str) -> Option<&Node> {
        self.nodes.iter().find(|n| n.id == id)
    }
    /// Whether the saved node positions are left to right (the `ui.direction` marker).
    pub fn is_lr(&self) -> bool {
        self.ui
            .as_ref()
            .and_then(|ui| ui.direction.as_deref())
            .is_some_and(|d| d == DIRECTION_LR)
    }
    /// The workflow's trigger nodes, in definition order. Any of them can start a run.
    pub fn triggers(&self) -> Vec<&Node> {
        self.nodes.iter().filter(|n| n.is_trigger()).collect()
    }
    /// The first trigger node, when there is one.
    pub fn first_trigger(&self) -> Option<&Node> {
        self.nodes.iter().find(|n| n.is_trigger())
    }
}

/// The workflow with its per-run values laid over the stored input values and cleared, so what
/// runs reads one set of values.
pub fn with_run_values(w: &Workflow) -> Workflow {
    let mut out = w.clone();
    if let Some(run_values) = out.run_values.take() {
        for (name, value) in run_values.iter() {
            out.input_values.insert(name.clone(), value.clone());
        }
    }
    out
}

// A workflow (or part of one) read from a run file is parsed as leniently as workflows.json is.
macro_rules! lenient_deserialize {
    ($ty:ty, $parse:path) => {
        impl<'de> Deserialize<'de> for $ty {
            fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
                let raw = super::ordered::Json::deserialize(d)?;
                $parse(&raw).ok_or_else(|| {
                    serde::de::Error::custom(concat!("unreadable ", stringify!($ty)))
                })
            }
        }
    };
}
lenient_deserialize!(Workflow, parse::parse_workflow);
lenient_deserialize!(Node, parse::parse_node);
lenient_deserialize!(Edge, parse::parse_edge);

/// `value` as JSON with object keys sorted, like the React app's `stableJson`.
pub(crate) fn stable_json(value: &Value) -> String {
    match value {
        Value::Array(items) => {
            let items: Vec<String> = items.iter().map(stable_json).collect();
            format!("[{}]", items.join(","))
        }
        Value::Object(map) => {
            let mut members: Vec<(&String, &Value)> = map.iter().collect();
            members.sort_by(|a, b| a.0.cmp(b.0));
            let members: Vec<String> = members
                .into_iter()
                .map(|(k, v)| {
                    format!(
                        "{}:{}",
                        serde_json::to_string(k).unwrap_or_default(),
                        stable_json(v)
                    )
                })
                .collect();
            format!("{{{}}}", members.join(","))
        }
        other => serde_json::to_string(other).unwrap_or_else(|_| "null".into()),
    }
}

/// A stable serialization of everything in a workflow that decides what runs or what reaches an
/// agent: its name, description, input declarations, nodes (every field but their board position)
/// and edges, loops included. Two workflows with the same fingerprint show the same review. It
/// leaves out input values and the fields the app manages (enabled, schedule state, run state).
pub fn definition_fingerprint(w: &Workflow) -> String {
    let nodes: Vec<Value> = w
        .nodes
        .iter()
        .map(|node| {
            let mut value = serde_json::to_value(node).unwrap_or(Value::Null);
            if let Value::Object(map) = &mut value {
                map.remove("ui");
            }
            value
        })
        .collect();
    let mut root = serde_json::Map::new();
    root.insert("name".into(), Value::String(w.name.clone()));
    root.insert(
        "description".into(),
        Value::String(w.description.clone().unwrap_or_default()),
    );
    root.insert(
        "inputs".into(),
        serde_json::to_value(&w.inputs).unwrap_or(Value::Null),
    );
    if !w.requires.is_empty() {
        root.insert(
            "requires".into(),
            serde_json::to_value(&w.requires).unwrap_or(Value::Null),
        );
    }
    root.insert("nodes".into(), Value::Array(nodes));
    root.insert(
        "edges".into(),
        serde_json::to_value(&w.edges).unwrap_or(Value::Null),
    );
    // What the AI decisions run on, who is told and what the summary says all reach a model or
    // a person, so a change to one needs a new review.
    if let Some(model) = &w.decision_model {
        root.insert(
            "decisionModel".into(),
            serde_json::to_value(model).unwrap_or(Value::Null),
        );
    }
    if let Some(report_to) = w.report_to.as_ref().filter(|r| r.mono.is_some()) {
        root.insert(
            "reportTo".into(),
            serde_json::to_value(report_to).unwrap_or(Value::Null),
        );
    }
    if let Some(template) = w.summary_template.as_deref().filter(|t| !t.is_empty()) {
        root.insert(
            "summaryTemplate".into(),
            Value::String(template.to_string()),
        );
    }
    // Where runs work and how many go at once decide what a run can touch.
    if !w.workspace.is_default() {
        root.insert(
            "workspace".into(),
            serde_json::to_value(&w.workspace).unwrap_or(Value::Null),
        );
    }
    if !w.concurrency.is_default() {
        root.insert(
            "concurrency".into(),
            serde_json::to_value(w.concurrency).unwrap_or(Value::Null),
        );
    }
    stable_json(&Value::Object(root))
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NodeStatus {
    Pending,
    Running,
    WaitingApproval,
    /// A "Wait for me" session whose first turn ended: the user chats on and then continues or
    /// fails the step. Not the same as `WaitingApproval` (a tool permission).
    WaitingForUser,
    Done,
    Failed,
    Skipped,
    Stopped,
}

/// The parts of a For each branch key.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Branch {
    pub group_id: NodeId,
    pub index: usize,
    /// Set on archived branches.
    pub round: Option<u32>,
}

fn digits(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|b| b.is_ascii_digit())
}

/// `^(.+)#(\d+)(?:@r(\d+))?$`, or None for the top level.
pub fn parse_branch(branch: &str) -> Option<Branch> {
    // The tail holds no `#`, so the last `#` is the only one a greedy `.+` can stop at.
    let (group, tail) = branch.rsplit_once('#')?;
    if group.is_empty() || group.contains(['\n', '\r', '\u{2028}', '\u{2029}']) {
        return None;
    }
    let (index, round) = match tail.split_once("@r") {
        Some((index, round)) if digits(round) => (index, Some(round.parse().ok()?)),
        Some(_) => return None,
        None => (tail, None),
    };
    digits(index).then_some(())?;
    Some(Branch {
        group_id: group.to_string(),
        index: index.parse().ok()?,
        round,
    })
}

pub fn is_archived_branch(branch: &str) -> bool {
    parse_branch(branch).is_some_and(|b| b.round.is_some())
}

pub fn archived_branch(branch: &str, round: u32) -> BranchKey {
    format!("{branch}@r{round}")
}

/// Where the item list of a For each is kept once a loop round archived it.
pub fn archived_items_key(group_id: &str, round: u32) -> NodeId {
    format!("{group_id}@r{round}")
}

/// What a decider chose, and why.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Decision {
    pub value: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

/// What a node did in one round of a loop, kept when the loop sends the branch back for the next.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoundRecord {
    pub round: u32,
    pub status: NodeStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NodeRun {
    pub node_id: NodeId,
    pub branch: BranchKey,
    pub status: NodeStatus,
    pub round: u32,
    /// The persisted id of the session the node ran in (a string in both apps).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub session_id: Option<String>,
    /// The loop session (round 2 or later) of a node that was interrupted while running, so a
    /// resumed run continues in it instead of opening a new session and worktree.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reuse_session_id: Option<String>,
    /// The sessions this node had when the run was interrupted and that a resumed run does not
    /// continue (it starts a new one). Kept so a cloud step can go on in the instance of its
    /// earlier session and the report can list that instance.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub interrupted_sessions: Option<Vec<String>>,
    /// Key order is kept as written (a prompt shows it), so this is not a serde_json map.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub output: Option<OrderedMap<Json>>,
    /// What the node's session wrote through `workflow.output` this round, last value per field.
    /// Cleared when the node starts a round; the runner merges it into `output` at the end.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub pending_output: Option<OrderedMap<Json>>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub decision: Option<Decision>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub started_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasked: Option<bool>,
    /// The rounds a loop already finished, oldest first. Never includes the current round.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub history: Option<Vec<RoundRecord>>,
}

impl NodeRun {
    pub fn new(node_id: impl Into<NodeId>, branch: impl Into<BranchKey>) -> Self {
        Self {
            node_id: node_id.into(),
            branch: branch.into(),
            status: NodeStatus::Pending,
            round: 1,
            session_id: None,
            reuse_session_id: None,
            interrupted_sessions: None,
            output: None,
            pending_output: None,
            error: None,
            decision: None,
            started_at: None,
            ended_at: None,
            reasked: None,
            history: None,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RunStatus {
    /// Waiting for a free slot (`concurrency.max` runs are going). Starts on its own, first in,
    /// first out.
    Queued,
    Running,
    Done,
    DoneWithFailures,
    Failed,
    Stopped,
    Interrupted,
    /// Never started: the queue was full (an event over the cap). `error` says why.
    Skipped,
}

/// The worktree a run (or one For each branch of it) works in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunWorktree {
    pub path: String,
    pub branch: String,
    /// The commit the worktree started from, to tell what the run changed.
    pub base: String,
    /// When the user removed the folder. The record stays, so the run page still names the branch
    /// (its commits are kept).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub removed_at: Option<u64>,
}

impl RunWorktree {
    /// Whether the folder is gone (removed from the run page).
    pub fn is_removed(&self) -> bool {
        self.removed_at.is_some()
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowRun {
    pub id: String,
    pub workflow_id: WorkflowId,
    /// Snapshot of the definition the run started from.
    pub definition: Workflow,
    pub status: RunStatus,
    pub started_at: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ended_at: Option<u64>,
    /// Who started the run and through which trigger node.
    pub caller: RunCaller,
    /// The lists For each nodes expanded, by node id (`<id>@r<round>` for an archived round).
    #[serde(default)]
    pub items: BTreeMap<NodeId, Vec<Json>>,
    #[serde(default)]
    pub nodes: Vec<NodeRun>,
    /// When the run summary was delivered.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub report_posted_at: Option<u64>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    /// The cloud instances this run started or went on in, kept as they start so a run that is
    /// interrupted and never resumed still says what it left running. Old records have none.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub cloud_instances: Vec<RunInstance>,
    /// The summary the run ended with (what `compose_report` made of it), kept for the run page.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<RunSummary>,
    /// The run's number in its workflow (the `<n>` of `wf/<slug>-<n>`).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub number: Option<u64>,
    /// When the run was queued; set while it waits and kept after it starts.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub queued_at: Option<u64>,
    /// The run's own worktree (`workspace.mode = perRun`). The run owns it: it is kept after the
    /// run and removed only by the user.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub worktree: Option<RunWorktree>,
    /// The worktree of each For each branch (`<forEach>#<index>`), forked from the run's.
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub branch_worktrees: BTreeMap<BranchKey, RunWorktree>,
}

/// The summary of a finished run: a title line and Markdown text.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
pub struct RunSummary {
    pub title: String,
    pub text: String,
}

/// One cloud instance a run used. The app never stops or deletes it on its own.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RunInstance {
    /// `codespace` or `sandbox`.
    pub kind: String,
    pub name: String,
    /// The run made this instance (and did not only go on in one that existed).
    #[serde(default)]
    pub created: bool,
    /// The nodes that ran on it.
    #[serde(default)]
    pub nodes: Vec<NodeId>,
    /// The persisted ids of the sessions that ran on it.
    #[serde(default)]
    pub sessions: Vec<String>,
}

impl RunInstance {
    /// "Codespace octo-run" / "Sandbox claude-app".
    pub fn label(&self) -> String {
        let kind = match self.kind.as_str() {
            "codespace" => "Codespace",
            "sandbox" => "Sandbox",
            other => other,
        };
        format!("{kind} {}", self.name)
    }
}

impl WorkflowRun {
    /// The instances an interrupted run left running, as one sentence for the board and the
    /// answers of `workflows.get`, `workflows.stop` and `workflows.remove`. `None` unless the
    /// run is interrupted and used an instance. Nothing here stops or deletes anything.
    pub fn left_instances_warning(&self) -> Option<String> {
        if self.status != RunStatus::Interrupted || self.cloud_instances.is_empty() {
            return None;
        }
        Some(left_instances_text("This run", &self.cloud_instances))
    }
}

/// The distinct instances (by kind and name) the interrupted runs among `runs` used. Their
/// reports never came, so removing the workflow would make the app forget them.
pub fn interrupted_instances(runs: &[WorkflowRun]) -> Vec<RunInstance> {
    let mut out: Vec<RunInstance> = Vec::new();
    for run in runs.iter().filter(|r| r.status == RunStatus::Interrupted) {
        for instance in &run.cloud_instances {
            if !out
                .iter()
                .any(|o| o.kind == instance.kind && o.name == instance.name)
            {
                out.push(instance.clone());
            }
        }
    }
    out
}

/// "This run left 2 cloud instances running: Codespace a, Sandbox b. ..." for `who`.
pub fn left_instances_text(who: &str, instances: &[RunInstance]) -> String {
    let labels: Vec<String> = instances.iter().map(RunInstance::label).collect();
    format!(
        "{who} left {} cloud instance{} running: {}. The app never stops or deletes them on its own: stop or delete them from Cloud workspaces.",
        instances.len(),
        if instances.len() == 1 { "" } else { "s" },
        labels.join(", ")
    )
}
