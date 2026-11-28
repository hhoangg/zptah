//! Event trigger matching: which Inbox items a workflow's event trigger reacts to,
//! the stable dedup key of an item, and the item recorded in the run scope.
//! Ported from `automations.rs`; the actor and branch filters are dropped.
use crate::inbox::{is_tracker, provider_label, Item};
use serde::{Deserialize, Serialize};
use serde_json::{json, Value};

/// The filter of one event trigger node.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct EventTrigger {
    pub source: String,
    pub event: String,
    /// The project this trigger is limited to. The caller resolves it (a path, or
    /// the workflow's own project) and passes the result to [`matches`].
    #[serde(skip_serializing_if = "Option::is_none")]
    pub project: Option<String>,
    /// Repositories (GitHub, GitLab, Azure DevOps) or ClickUp space ids. Empty means any.
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub repos: Vec<String>,
}

/// Every supported (source, event id, label) triple.
pub const EVENTS: &[(&str, &str, &str)] = &[
    ("github", "draft_opened", "Draft opened"),
    ("github", "pull_request_opened", "Pull request opened"),
    ("github", "issue_opened", "Issue opened"),
    ("gitlab", "merge_request_opened", "Merge request opened"),
    ("gitlab", "issue_opened", "Issue opened"),
    ("linear", "issue_created", "Issue created"),
    ("jira", "issue_created", "Issue appeared"),
    ("clickup", "issue_created", "Task appeared"),
    (
        "azuredevops",
        "pull_request_appeared",
        "Pull request appeared",
    ),
    ("azuredevops", "work_item_appeared", "Work item appeared"),
];

/// The events one source offers, as (id, label).
pub fn events_for(source: &str) -> Vec<(&'static str, &'static str)> {
    EVENTS
        .iter()
        .filter(|(s, _, _)| *s == source)
        .map(|(_, id, label)| (*id, *label))
        .collect()
}

/// The label of one event: "Pull request opened". The raw id when the pair is unknown.
pub fn event_label<'a>(source: &str, event: &'a str) -> &'a str {
    EVENTS
        .iter()
        .find(|(s, e, _)| *s == source && *e == event)
        .map_or(event, |(_, _, label)| label)
}

impl EventTrigger {
    /// One line for the board and the Mono tools: "GitHub: Pull request opened".
    pub fn summary(&self) -> String {
        format!(
            "{}: {}",
            provider_label(&self.source),
            event_label(&self.source, &self.event)
        )
    }
}

/// Reject a source and event pair outside [`EVENTS`].
pub fn check_event(source: &str, event: &str) -> Result<(), String> {
    if EVENTS.iter().any(|(s, e, _)| *s == source && *e == event) {
        return Ok(());
    }
    if !EVENTS.iter().any(|(s, _, _)| *s == source) {
        return Err(format!("Unknown event source \"{source}\""));
    }
    Err(format!("Source \"{source}\" has no event \"{event}\""))
}

/// The event an Inbox item represents, or None when no trigger can fire for it.
pub fn event_of(item: &Item) -> Option<&'static str> {
    Some(
        match (item.provider.as_str(), item.kind.as_str(), item.draft) {
            ("github", "pr", true) => "draft_opened",
            ("github", "pr", false) => "pull_request_opened",
            ("github" | "gitlab", "issue", _) => "issue_opened",
            ("gitlab", "pr", _) => "merge_request_opened",
            ("linear" | "jira" | "clickup", _, _) => "issue_created",
            ("azuredevops", "pr", _) => "pull_request_appeared",
            ("azuredevops", "issue", _) => "work_item_appeared",
            _ => return None,
        },
    )
}

/// A stable dedup key for an item. It never changes across polls and is distinct
/// per item. The event is not part of it: one item has exactly one event.
pub fn event_key(item: &Item) -> String {
    let identity = if is_tracker(&item.provider) {
        let id = if !item.id.is_empty() {
            item.id.clone()
        } else if !item.identifier.is_empty() {
            item.identifier.clone()
        } else {
            item.number.to_string()
        };
        let noun = if item.provider == "clickup" {
            "task"
        } else {
            "issue"
        };
        format!("{}:{noun}:{id}", item.provider)
    } else {
        format!(
            "{}:{}:{}:{}",
            item.provider, item.kind, item.repo, item.number
        )
    };
    identity
        .trim()
        .to_lowercase()
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, ':' | '_' | '-' | '.' | '/') {
                c
            } else {
                '_'
            }
        })
        .take(400)
        .collect()
}

fn path_key(path: &str) -> String {
    let normalized = path.trim().replace('\\', "/");
    let normalized = normalized.trim_end_matches('/');
    let normalized = if normalized.is_empty() {
        "/"
    } else {
        normalized
    };
    if normalized.as_bytes().get(1) == Some(&b':') || normalized.starts_with("//") {
        normalized.to_lowercase()
    } else {
        normalized.to_string()
    }
}

/// Whether the trigger fires for the item. `project` is the resolved project path
/// (falls back to the trigger's own `project` when empty). A tracker item that
/// carries no project path matches any project; every other item must be in it.
/// With no project at all, only such tracker items match (fail closed).
pub fn matches(trigger: &EventTrigger, item: &Item, project: &str) -> bool {
    if trigger.source != item.provider || event_of(item) != Some(trigger.event.as_str()) {
        return false;
    }
    let project = if project.trim().is_empty() {
        trigger.project.as_deref().unwrap_or("")
    } else {
        project
    };
    let tracker_without_project = is_tracker(&item.provider) && item.project_path.trim().is_empty();
    if !tracker_without_project
        && (project.trim().is_empty() || path_key(&item.project_path) != path_key(project))
    {
        return false;
    }
    let repos: Vec<_> = trigger
        .repos
        .iter()
        .map(|repo| repo.trim())
        .filter(|repo| !repo.is_empty())
        .collect();
    // A ClickUp filter holds space ids.
    let scope = if item.provider == "clickup" {
        item.team_id.trim()
    } else {
        item.repo.trim()
    };
    repos.is_empty() || repos.iter().any(|repo| repo.eq_ignore_ascii_case(scope))
}

/// The item recorded in a run's `trigger.item` scope.
pub fn trigger_item(item: &Item) -> Value {
    let provider = provider_label(&item.provider);
    let kind = if item.provider == "clickup" {
        "task"
    } else if is_tracker(&item.provider) || item.kind != "pr" {
        "issue"
    } else if item.provider == "gitlab" {
        "merge request"
    } else {
        "pull request"
    };
    let reference = if is_tracker(&item.provider) {
        if item.identifier.trim().is_empty() {
            format!("{provider} #{}", item.number)
        } else {
            item.identifier.trim().to_string()
        }
    } else {
        format!("#{}", item.number)
    };
    let title = if item.title.trim().is_empty() {
        if is_tracker(&item.provider) {
            reference.clone()
        } else {
            format!("{provider} {kind} #{}", item.number)
        }
    } else {
        item.title.trim().to_string()
    };
    json!({
        "source": item.provider,
        "kind": kind,
        "ref": reference,
        "title": title,
        "url": item.url.trim(),
        "repo": item.repo.trim(),
        // What a session links to when its node sets `linkWorkItem` (the Inbox's own work item).
        "workItem": item.work_item(),
        // The claim's key, so a crash between making a run and recording it is repaired at startup.
        "eventKey": event_key(item),
    })
}

/// The work item a run's `trigger.item` carries, for a session that links it.
pub fn work_item_of(item: &serde_json::Value) -> Option<crate::session_groups::WorkItem> {
    serde_json::from_value(item.get("workItem")?.clone()).ok()
}

/// The project text an event trigger is limited to: its own `project`, else the workflow's
/// `workspace.project`. `{{inputs.X}}` is read from the stored input values (D29). Empty when
/// neither names one, or the input has no value.
pub fn trigger_project(w: &super::model::Workflow, trigger: &EventTrigger) -> String {
    let text = trigger
        .project
        .as_deref()
        .filter(|p| !p.trim().is_empty())
        .unwrap_or(&w.workspace.project)
        .trim();
    let named = text
        .strip_prefix("{{")
        .and_then(|rest| rest.strip_suffix("}}"))
        .map(str::trim)
        .and_then(|reference| reference.strip_prefix("inputs."));
    match named {
        Some(name) => match w.input_values.get(name) {
            Some(super::model::InputValue::String(path)) => path.trim().to_string(),
            _ => String::new(),
        },
        None if text.contains("{{") => String::new(),
        None => text.to_string(),
    }
}

/// The first trigger of `w` (in definition order) that fires for `item`: one run per workflow
/// per item, so the first match wins (D28). `projects` are the app's project paths; a project
/// text naming one by folder name is resolved to its path.
pub fn matching_trigger<'a>(
    w: &'a super::model::Workflow,
    item: &Item,
    projects: &[String],
) -> Option<&'a super::model::Node> {
    w.nodes.iter().find(|node| {
        let super::model::NodeKind::Event(trigger) = &node.kind else {
            return false;
        };
        if node.parent.is_some() {
            return false;
        }
        let text = trigger_project(w, trigger);
        let path = crate::mono::resolve_project(projects, &text).unwrap_or(text);
        matches(trigger, item, &path)
    })
}
