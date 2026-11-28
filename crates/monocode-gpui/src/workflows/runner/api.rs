//! The seam between a run and the app: what a run asks of its host, and the values that cross it.
use super::Fut;
use monocode_core::mono::{
    habits,
    workflows::{command::CommandResult, model::WorkflowRun},
};

/// The waits a run uses; tests shorten them.
#[derive(Clone, Copy, Debug)]
pub(crate) struct Limits {
    /// How often a watched turn is looked at.
    pub poll_ms: u64,
    /// How long an AI decision may take.
    pub decision_timeout_ms: u64,
    /// How long a node's session may wait for an approval.
    pub approval_wait_ms: u64,
}

impl Default for Limits {
    fn default() -> Self {
        Self {
            poll_ms: 2_000,
            decision_timeout_ms: 3 * 60_000,
            approval_wait_ms: habits::APPROVAL_WAIT_MS,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(crate) enum Status {
    Completed,
    Failed,
    Cancelled,
}

/// How a turn ended (`ControlOutcome`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct Outcome {
    pub status: Status,
    pub text: String,
    pub error: Option<String>,
}

impl Outcome {
    pub(crate) fn completed(text: impl Into<String>) -> Self {
        Self {
            status: Status::Completed,
            text: text.into(),
            error: None,
        }
    }
    pub(crate) fn failed(error: impl Into<String>) -> Self {
        Self {
            status: Status::Failed,
            text: String::new(),
            error: Some(error.into()),
        }
    }
    pub(crate) fn cancelled(error: impl Into<String>) -> Self {
        Self {
            status: Status::Cancelled,
            text: String::new(),
            error: Some(error.into()),
        }
    }
}

/// A session a node asks for (`SessionRequest`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionRequest {
    /// One of the app's project paths.
    pub project: String,
    pub prompt: String,
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// `workspaceMode: "worktree"`: a new worktree for the session.
    pub new_worktree: bool,
    pub worktree_cwd: Option<String>,
    pub parent_session_id: Option<String>,
    /// The sidebar folder the session is filed in: the workflow's own, made on demand and shared
    /// by all its runs.
    pub folder_name: Option<String>,
    /// `Some` for a cloud step: the session runs in a codespace or sandbox instead of here.
    pub target: Option<CloudRequest>,
}

/// The cloud instance a step asks for, with its instance already resolved by the run.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudRequest {
    /// `codespace` or `sandbox`.
    pub kind: String,
    pub instance: CloudInstance,
    /// The other keys of the node's target (branch, machine, ...), with the templates of the
    /// string options already filled in and checked.
    pub options: serde_json::Map<String, serde_json::Value>,
    /// The Claude account a NEW instance runs with, as the node wrote it (`"default"`, an id or
    /// a label); `None` for an existing instance or a node without one. Whether it exists and
    /// carries a setup token or API key is decided by the start, before anything is created.
    pub account: Option<String>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) enum CloudInstance {
    /// Create one.
    New,
    /// The instance the session runs on, by persisted session id.
    OfSession(String),
}

/// The cloud instance a session runs on, for the report.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct CloudInstanceInfo {
    /// `codespace` or `sandbox`.
    pub kind: String,
    /// `None` while the instance is still being created and has no name yet.
    pub name: Option<String>,
}

pub(crate) struct StartedSession {
    /// The persisted session id, as run files store it.
    pub session_id: String,
    /// Resolves when the first turn settles.
    pub settled: Fut<Outcome>,
}

/// A session the summary lists.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct SessionInfo {
    pub session_id: String,
    /// The app's own id of the session, when it is known to this app.
    pub local_id: Option<i64>,
    pub cwd: String,
    pub title: String,
    pub harness: String,
    pub model: Option<String>,
}

/// A run's summary, handed to the app to deliver: a notification attached to the run's last
/// session, and the chats of the Monos that asked for it (`reportTo`, the Mono that started the
/// run). The run record itself already holds it (`WorkflowRun::summary`).
#[derive(Clone, Debug, PartialEq)]
pub(crate) struct Summary {
    /// The run as it ended, with its summary set.
    pub run: WorkflowRun,
    pub title: String,
    pub text: String,
    /// The sessions the run started, in the order of its nodes.
    pub launches: Vec<SessionInfo>,
}

/// An AI decision (If, Switch or loop edge) asked of a one-shot, no-tools call.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct DecisionRequest {
    /// What `decision_model_for` found. A part it leaves unset (a session node that used the app's
    /// defaults) is the app's default for a new session, filled in by the host.
    pub harness: Option<String>,
    pub model: Option<String>,
    pub effort: Option<String>,
    /// An app project to run in and to pick the account from, when the run has one.
    pub project: Option<String>,
    pub prompt: String,
    pub timeout_ms: u64,
}

/// Everything a run needs from the app (`MonoWorkflowHost`).
pub(crate) trait WorkflowHost {
    /// Starts a visible session in an app project, filed in the workflow's sidebar folder, with
    /// no Mono involved.
    fn start_session(&self, req: SessionRequest) -> Fut<Result<StartedSession, String>>;
    /// Sends a follow-up turn to an existing session; resolves when it settles.
    fn send(&self, session_id: &str, prompt: String) -> Fut<Outcome>;
    fn stop(&self, session_id: &str) -> Fut<Result<(), String>>;
    /// The last thing the session said this turn (empty when it said nothing).
    fn final_reply(&self, session_id: &str) -> String;
    /// Whether the session is waiting on the user (an approval or a question). The session itself
    /// shows it, and the app raises its usual notices.
    fn waiting_approval(&self, session_id: &str) -> bool;
    /// The run is done with the session: whatever the host kept to watch it can go.
    fn release_session(&self, session_id: &str);
    fn session_info(&self, session_id: &str) -> Option<SessionInfo>;
    /// The cloud instance the session runs on; `None` for a session on this computer. Reads the
    /// store only: it never wakes, starts or asks the provider anything.
    fn cloud_instance(&self, _session_id: &str) -> Option<CloudInstanceInfo> {
        None
    }
    /// Runs a shell command; `id` names it for `cancel_command` while it runs.
    fn run_command(
        &self,
        cwd: String,
        command: String,
        timeout_ms: u64,
        id: String,
    ) -> Fut<Result<CommandResult, String>>;
    /// Kills the running command `id`; false when no such command runs (yet).
    fn cancel_command(&self, id: &str) -> Fut<Result<bool, String>>;
    /// A hidden one-shot, no-tools model call; resolves with its reply. Dropping the future
    /// cancels the call.
    fn decide(&self, request: DecisionRequest) -> Fut<Result<String, String>>;
    /// Delivers the summary of a run (see `Summary`).
    fn post(&self, summary: Summary);
    /// Resolves a `{{inputs.x}}` value to one of the app's project paths.
    fn project_path(&self, input: &str) -> Option<String>;
    /// The working directory of a session (its worktree, else its project).
    fn worktree_of(&self, session_id: &str) -> Option<String>;
    /// Writes the run into the workflow's run history.
    fn save_run(&self, run: WorkflowRun) -> Fut<Result<(), String>>;
}
