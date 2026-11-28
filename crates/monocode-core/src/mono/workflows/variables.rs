//! The variables a node's templates may use, for the inspector's "Available variables" list.
//! The scope comes from `validate::node_scope` and the names from `validate::output_fields`,
//! the very rules validation applies, so the list never offers a variable validation refuses
//! (a test validates every offered variable in the node's own prompt).
//!
//! Adding a scope variable is one line in `SCOPE_VARS`.
use super::model::{NodeKind, OutputFieldType, Workflow};
use super::output::{COMMAND_BUILTINS, SESSION_BUILTINS};
use super::validate::{node_scope, output_fields};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum VariableKind {
    Text,
    Number,
    Boolean,
    List,
    Object,
    /// The shape is not declared (an event's item, a loop item).
    Any,
}

impl VariableKind {
    pub fn name(self) -> &'static str {
        match self {
            Self::Text => "text",
            Self::Number => "number",
            Self::Boolean => "boolean",
            Self::List => "list",
            Self::Object => "object",
            Self::Any => "any",
        }
    }
}

impl From<OutputFieldType> for VariableKind {
    fn from(ty: OutputFieldType) -> Self {
        match ty {
            OutputFieldType::String => Self::Text,
            OutputFieldType::Number => Self::Number,
            OutputFieldType::Boolean => Self::Boolean,
            OutputFieldType::List => Self::List,
            OutputFieldType::Object => Self::Object,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum VariableGroup {
    Trigger,
    Inputs,
    Run,
    /// The outputs of the upstream node with this id.
    Step(String),
    /// The loop variable of an enclosing For each, by its name.
    Loop(String),
    /// What an event brought.
    Item,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Variable {
    /// The text to insert, braces included: `{{inputs.project}}`.
    pub path: String,
    pub label: String,
    pub group: VariableGroup,
    pub kind: VariableKind,
    /// How many characters before the end of `path` the caret goes after it is inserted: 2 for
    /// `{{item.}}`, so the field name is typed next. 0 for a complete variable.
    pub caret_back: usize,
}

/// Which workflows a scope variable applies to.
#[derive(Clone, Copy)]
enum When {
    Always,
    /// The workflow has an event trigger.
    Event,
}

/// Whether the workflow can start from an event (`trigger.item.*` exists only then).
/// Hook: the event trigger node kind does not exist yet. When it is added, match it here and
/// nothing else in this file changes.
fn has_event_trigger(_w: &Workflow) -> bool {
    false
}

impl When {
    fn holds(self, w: &Workflow) -> bool {
        match self {
            Self::Always => true,
            Self::Event => has_event_trigger(w),
        }
    }
}

/// The variables every node of a workflow has, besides inputs, steps and loop variables:
/// (group, path without braces, label, kind, when). One line each; `run` entries must match
/// `validate::RUN_VARS` (a test checks it) and `trigger` ones `validate::TRIGGER_VARS`.
const SCOPE_VARS: &[(&str, &str, &str, VariableKind, When)] = &[
    (
        "trigger",
        "trigger.node",
        "Id of the trigger that started the run",
        VariableKind::Text,
        When::Always,
    ),
    (
        "trigger",
        "trigger.kind",
        "What started the run (schedule, manual, ...)",
        VariableKind::Text,
        When::Always,
    ),
    (
        "run",
        "run.id",
        "Id of this run",
        VariableKind::Text,
        When::Always,
    ),
    (
        "run",
        "run.round",
        "Current round of the loop",
        VariableKind::Number,
        When::Always,
    ),
    (
        "run",
        "run.startedAt",
        "When the run started",
        VariableKind::Text,
        When::Always,
    ),
    (
        "run",
        "run.worktree",
        "Folder of the run's worktree (empty without one)",
        VariableKind::Text,
        When::Always,
    ),
    (
        "run",
        "run.branch",
        "Branch of the run's worktree (empty without one)",
        VariableKind::Text,
        When::Always,
    ),
    // Event items: the validator takes any path under `trigger.item`.
    (
        "item",
        "trigger.item",
        "The item the event brought",
        VariableKind::Object,
        When::Event,
    ),
];

fn group_of(name: &str) -> VariableGroup {
    match name {
        "trigger" => VariableGroup::Trigger,
        "run" => VariableGroup::Run,
        _ => VariableGroup::Item,
    }
}

fn braces(path: &str) -> String {
    format!("{{{{{path}}}}}")
}

fn field_kind(node: &super::model::Node, name: &str) -> VariableKind {
    match &node.kind {
        NodeKind::Session { output, .. } => output
            .get(name)
            .map(|f| f.ty)
            .or_else(|| {
                SESSION_BUILTINS
                    .iter()
                    .find(|(n, _)| *n == name)
                    .map(|(_, ty)| *ty)
            })
            .map_or(VariableKind::Any, Into::into),
        NodeKind::Command { .. } => COMMAND_BUILTINS
            .iter()
            .find(|(n, _)| *n == name)
            .map_or(VariableKind::Any, |(_, ty)| (*ty).into()),
        NodeKind::ForEach { .. } => VariableKind::List,
        _ => VariableKind::Any,
    }
}

/// The variables the templates of node `node_id` may use, in the order the list shows them:
/// trigger, inputs, run, loop variables, then the outputs of each upstream node in workflow
/// order. Empty when the workflow has no such node.
pub fn available_variables(w: &Workflow, node_id: &str) -> Vec<Variable> {
    let Some((steps, items)) = node_scope(w, node_id) else {
        return Vec::new();
    };
    let mut out = Vec::new();
    let mut push = |path: &str, label: String, group: VariableGroup, kind: VariableKind| {
        // A path ending in `.` waits for a field name: the caret goes before the braces.
        let caret_back = if path.ends_with('.') { 2 } else { 0 };
        out.push(Variable {
            path: braces(path),
            label,
            group,
            kind,
            caret_back,
        });
    };
    for (group, path, label, kind, when) in SCOPE_VARS {
        if *group == "trigger" && when.holds(w) {
            push(path, (*label).into(), group_of(group), *kind);
        }
    }
    for (name, input) in w.inputs.iter() {
        let label = input
            .description
            .clone()
            .filter(|d| !d.is_empty())
            .unwrap_or_else(|| format!("Input {name}"));
        let kind = match input.ty {
            super::model::InputType::Boolean => VariableKind::Boolean,
            super::model::InputType::Number => VariableKind::Number,
            _ => VariableKind::Text,
        };
        push(
            &format!("inputs.{name}"),
            label,
            VariableGroup::Inputs,
            kind,
        );
    }
    for (group, path, label, kind, when) in SCOPE_VARS {
        if *group != "trigger" && when.holds(w) {
            push(path, (*label).into(), group_of(group), *kind);
        }
    }
    for name in &items {
        let group = VariableGroup::Loop(name.clone());
        push(
            name,
            "The current item".into(),
            group.clone(),
            VariableKind::Any,
        );
        push(
            &format!("{name}."),
            "A field of the current item".into(),
            group,
            VariableKind::Any,
        );
    }
    let mut seen: Vec<&str> = Vec::new();
    for node in &w.nodes {
        if !steps.contains(&node.id) || seen.contains(&node.id.as_str()) {
            continue;
        }
        seen.push(node.id.as_str());
        for field in output_fields(node) {
            push(
                &format!("{}.{field}", node.id),
                field.clone(),
                VariableGroup::Step(node.id.clone()),
                field_kind(node, &field),
            );
        }
    }
    out
}
