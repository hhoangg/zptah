//! Which workflow step a `workflow.output` call belongs to. The call carries no run id: the app
//! knows only the calling session, so it looks for the node whose ACTIVE session that is.
//!
//! A session is the active session of one node (in one For each branch, in one round) at a time.
//! A node that reuses its session across loop rounds is the same node, and a round ends before
//! the next starts, so a call is only ever accepted while its own round is still running.
//! Everything here is pure: the caller passes the live runs and the ids the session goes by.
use super::engine::is_asking;
use super::model::{
    BranchKey, NodeId, NodeKind, NodeRun, NodeStatus, RunStatus, WorkflowId, WorkflowRun,
};
use super::ordered::{Json, OrderedMap};
use super::output::check_output_value;

/// The step a session is running.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ActiveNode {
    pub workflow_id: WorkflowId,
    pub run_id: String,
    pub node_id: NodeId,
    /// The For each branch, empty at the top level.
    pub branch: BranchKey,
    pub round: u32,
}

/// Why a call cannot be placed.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ActiveError {
    /// The session is not, and was not, a workflow step's session.
    NotAStep,
    /// The session ran a step that is over (the step or its round finished, or the run ended).
    Ended,
    /// Two live steps claim the session: nothing is written, because either could be meant.
    Ambiguous,
}

impl std::fmt::Display for ActiveError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::NotAStep => "This session is not running a workflow step, so there is nothing to record. workflow.output is only for the session of a running workflow step.",
            Self::Ended => "The workflow step this session ran has already ended (or moved on to its next round), so the call was ignored.",
            Self::Ambiguous => "This session belongs to more than one running workflow step, so the call was refused.",
        })
    }
}

fn is_live(status: NodeStatus) -> bool {
    matches!(
        status,
        NodeStatus::Running | NodeStatus::WaitingApproval | NodeStatus::WaitingForUser
    )
}

/// Whether `r` is a session step doing its round right now: running (or waiting on the user)
/// and not already finished and only being asked about its loop.
fn is_active(run: &WorkflowRun, r: &NodeRun) -> bool {
    run.status == RunStatus::Running
        && is_live(r.status)
        && !is_asking(r)
        && run
            .definition
            .node(&r.node_id)
            .is_some_and(|n| matches!(n.kind, NodeKind::Session { .. }))
}

fn names(r: &NodeRun, ids: &[String]) -> bool {
    r.session_id.as_ref().is_some_and(|id| ids.contains(id))
}

fn named_before(r: &NodeRun, ids: &[String]) -> bool {
    r.reuse_session_id
        .as_ref()
        .is_some_and(|id| ids.contains(id))
        || r.interrupted_sessions
            .iter()
            .flatten()
            .any(|id| ids.contains(id))
        || r.history
            .iter()
            .flatten()
            .any(|h| h.session_id.as_ref().is_some_and(|id| ids.contains(id)))
}

/// The step `ids` is the active session of. `ids` are every id the calling session goes by: the
/// app's local id (`MONOCODE_APP_SESSION`) and the persisted id run files store, which differ
/// for a remote or shared-store session. `runs` are the runs the app is driving.
pub fn find_active(runs: &[&WorkflowRun], ids: &[String]) -> Result<ActiveNode, ActiveError> {
    let mut found: Vec<ActiveNode> = Vec::new();
    let mut ended = false;
    for run in runs {
        for r in &run.nodes {
            let current = names(r, ids);
            if current && is_active(run, r) {
                found.push(ActiveNode {
                    workflow_id: run.workflow_id.clone(),
                    run_id: run.id.clone(),
                    node_id: r.node_id.clone(),
                    branch: r.branch.clone(),
                    round: r.round,
                });
            } else if current || named_before(r, ids) {
                ended = true;
            }
        }
    }
    match found.len() {
        0 if ended => Err(ActiveError::Ended),
        0 => Err(ActiveError::NotAStep),
        1 => Ok(found.remove(0)),
        _ => Err(ActiveError::Ambiguous),
    }
}

/// `run` with `values` recorded as the output of `target`'s step: type-checked against the
/// fields the step declares, merged over what the session wrote before (the last value of a
/// field wins). The step must still be active in `run`. Err is the message for the caller.
pub fn record_output(
    run: &WorkflowRun,
    target: &ActiveNode,
    values: &Json,
) -> Result<WorkflowRun, String> {
    let Some(at) = run
        .nodes
        .iter()
        .position(|r| r.node_id == target.node_id && r.branch == target.branch)
    else {
        return Err(ActiveError::Ended.to_string());
    };
    let r = &run.nodes[at];
    if run.id != target.run_id || r.round != target.round || !is_active(run, r) {
        return Err(ActiveError::Ended.to_string());
    }
    let Some(NodeKind::Session { output, .. }) = run.definition.node(&r.node_id).map(|n| &n.kind)
    else {
        return Err(ActiveError::NotAStep.to_string());
    };
    let checked = check_output_value(output, values)?;
    let mut out = run.clone();
    let pending = out.nodes[at]
        .pending_output
        .get_or_insert_with(OrderedMap::new);
    for (name, value) in checked.iter() {
        pending.insert(name.clone(), value.clone());
    }
    Ok(out)
}
