//! Starter workflows: the examples the Workflows page offers under "New from template". Each is
//! a trigger (a schedule or an Inbox event) followed by one session step that carries the prompt.
//! They come from `templates.json`; picking one fills a new, paused workflow for a project.
use super::event::EventTrigger;
use super::model::{
    new_workflow_id, Checkout, Edge, Node, NodeKind, Ui, Workflow, Workspace, WorkspaceMode,
};
use super::ordered::OrderedMap;
use crate::mono::habits::Schedule;
use serde::Deserialize;
use std::sync::OnceLock;

/// What starts a template's workflow.
#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum TemplateTrigger {
    Schedule { schedule: Schedule },
    Event { source: String, event: String },
}

#[derive(Clone, Debug, PartialEq, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct WorkflowTemplate {
    pub id: String,
    /// Which icon the gallery draws (see the gallery for the names).
    pub icon: String,
    pub name: String,
    pub description: String,
    pub trigger: TemplateTrigger,
    pub prompt: String,
    #[serde(default)]
    pub harness: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
}

impl WorkflowTemplate {
    pub fn is_scheduled(&self) -> bool {
        matches!(self.trigger, TemplateTrigger::Schedule { .. })
    }

    /// "Weekdays at 09:00" or "GitHub: Pull request opened".
    pub fn trigger_label(&self) -> String {
        match &self.trigger {
            TemplateTrigger::Schedule { schedule } => schedule.label(),
            TemplateTrigger::Event { source, event } => EventTrigger {
                source: source.clone(),
                event: event.clone(),
                ..Default::default()
            }
            .summary(),
        }
    }

    /// A new, paused workflow for `project`: the trigger, then one session step. Each run works
    /// in its own worktree of the project. Event templates accept edits without asking, like
    /// scheduled ones, because nobody is there to approve them.
    pub fn instantiate(&self, project: &str, now: u64) -> Workflow {
        let kind = match &self.trigger {
            TemplateTrigger::Schedule { schedule } => NodeKind::Schedule {
                schedule: schedule.clone(),
            },
            TemplateTrigger::Event { source, event } => NodeKind::Event(EventTrigger {
                source: source.clone(),
                event: event.clone(),
                ..Default::default()
            }),
        };
        let mut trigger = Node::new("start", kind);
        trigger.ui = Some(Ui { x: 0.0, y: 0.0 });
        let mut step = Node::new(
            "work",
            NodeKind::Session {
                project: String::new(),
                prompt: self.prompt.clone(),
                checkout: Checkout::Run,
                output: OrderedMap::new(),
                mode: None,
                surface: None,
                harness: self.harness.clone(),
                model: self.model.clone(),
                effort: None,
                reuse_session: None,
                nest_under: None,
                timeout_minutes: None,
                target: None,
                account: None,
                access: None,
                folder: None,
                link_work_item: None,
            },
        );
        step.ui = Some(Ui { x: 360.0, y: 0.0 });
        let mut w = Workflow::new(new_workflow_id(), self.name.clone());
        w.description = Some(self.description.clone());
        w.nodes = vec![trigger, step];
        w.edges = vec![Edge::new("start", "work")];
        w.workspace = Workspace {
            mode: WorkspaceMode::PerRun,
            project: project.to_string(),
            base_branch: None,
        };
        w.created_at = now;
        w
    }
}

/// Every template, scheduled ones first, in the order of `templates.json`.
pub fn templates() -> &'static [WorkflowTemplate] {
    static TEMPLATES: OnceLock<Vec<WorkflowTemplate>> = OnceLock::new();
    TEMPLATES.get_or_init(|| {
        let mut all: Vec<WorkflowTemplate> =
            serde_json::from_str(include_str!("templates.json")).expect("templates.json parses");
        all.sort_by_key(|t| !t.is_scheduled());
        all
    })
}

pub fn template(id: &str) -> Option<&'static WorkflowTemplate> {
    templates().iter().find(|t| t.id == id)
}
