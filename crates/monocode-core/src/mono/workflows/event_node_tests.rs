//! The event trigger node and the session fields that go with it (`access`, `folder`,
//! `linkWorkItem`): reading, writing, validation, warnings, variables and the fingerprint.
use super::event::EventTrigger;
use super::fixtures::{node, workflow_from};
use super::model::{
    definition_fingerprint, NodeKind, Workflow, DEFAULT_SESSION_ACCESS, NODE_TYPES,
};
use super::models::ModelCatalog;
use super::validate::{unknown_project_issues, validate_workflow, workflow_warnings};
use super::variables::available_variables;
use super::yaml::{export_workflow, preview_import};
use serde_json::json;

fn workflow(trigger: &str, session_extra: &str) -> Workflow {
    workflow_from(&format!(
        r#"{{"id":"wf-evt00001","name":"On new PR","inputs":{{"repo":{{"type":"project"}}}},
            "inputValues":{{"repo":"/code/app"}},
            "nodes":[{trigger},
              {{"id":"work","type":"session","project":"{{{{inputs.repo}}}}","prompt":"Review {{{{trigger.item.title}}}}","checkout":"root","outputs":{{}}{session_extra}}}],
            "edges":[{{"from":"start","to":"work"}}],
            "workspace":{{"mode":"perRun","project":"{{{{inputs.repo}}}}"}},
            "enabled":false,"createdAt":1}}"#
    ))
}

const PR_TRIGGER: &str = r#"{"id":"start","type":"event","source":"github","event":"pull_request_opened","project":"{{inputs.repo}}","repos":["acme/app"]}"#;

fn event_workflow(session_extra: &str) -> Workflow {
    workflow(PR_TRIGGER, session_extra)
}

fn issues(w: &Workflow) -> Vec<String> {
    validate_workflow(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn warnings(w: &Workflow) -> Vec<String> {
    workflow_warnings(w)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

#[test]
fn an_event_node_is_a_trigger_and_reads_its_filter() {
    assert!(NODE_TYPES.contains(&"event"));
    let w = event_workflow("");
    let trigger = w.node("start").unwrap();
    assert!(trigger.is_trigger());
    assert_eq!(trigger.kind.type_name(), "event");
    let NodeKind::Event(event) = &trigger.kind else {
        panic!("not an event node")
    };
    assert_eq!(event.source, "github");
    assert_eq!(event.event, "pull_request_opened");
    assert_eq!(event.project.as_deref(), Some("{{inputs.repo}}"));
    assert_eq!(event.repos, ["acme/app"]);
    assert_eq!(issues(&w), Vec::<String>::new());
}

#[test]
fn an_event_node_with_a_filter_of_the_wrong_type_is_unreadable() {
    let bad = json!({"id": "start", "type": "event", "source": "github", "event": "issue_opened", "repos": "a/b"});
    assert!(super::parse::parse_node(&super::ordered::Json::from(&bad)).is_none());
    let bad = json!({"id": "start", "type": "event", "source": "github", "event": "issue_opened", "project": 3});
    assert!(super::parse::parse_node(&super::ordered::Json::from(&bad)).is_none());
}

#[test]
fn the_event_node_and_the_session_fields_survive_a_yaml_round_trip() {
    let w = event_workflow(r#","access":"auto","folder":"fld-1","linkWorkItem":true"#);
    let text = export_workflow(&w);
    assert!(text.contains("type: event"), "{text}");
    let preview = preview_import(&text, &ModelCatalog::unprobed()).unwrap();
    assert_eq!(preview.workflow.nodes, w.nodes);
    let NodeKind::Session {
        access,
        folder,
        link_work_item,
        ..
    } = &preview.workflow.nodes[1].kind
    else {
        panic!("not a session")
    };
    assert_eq!(access.as_deref(), Some("auto"));
    assert_eq!(folder.as_deref(), Some("fld-1"));
    assert_eq!(*link_work_item, Some(true));
}

#[test]
fn a_bad_source_and_event_pair_is_refused() {
    for (source, event, message) in [
        ("github", "pull_request_merged", "has no event"),
        ("github", "check_failed", "has no event"),
        ("gitlab", "draft_opened", "has no event"),
        ("bitbucket", "issue_created", "Unknown event source"),
        ("", "", "Unknown event source"),
    ] {
        let w = workflow(
            &format!(
                r#"{{"id":"start","type":"event","source":"{source}","event":"{event}","project":"/code/app"}}"#
            ),
            "",
        );
        let found = issues(&w);
        assert!(
            found.iter().any(|m| m.contains(message)),
            "{source}/{event}: {found:?}"
        );
    }
    for (source, event) in [
        ("github", "draft_opened"),
        ("github", "issue_opened"),
        ("gitlab", "merge_request_opened"),
        ("linear", "issue_created"),
        ("jira", "issue_created"),
        ("clickup", "issue_created"),
        ("azuredevops", "pull_request_appeared"),
        ("azuredevops", "work_item_appeared"),
    ] {
        let w = workflow(
            &format!(
                r#"{{"id":"start","type":"event","source":"{source}","event":"{event}","project":"/code/app"}}"#
            ),
            "",
        );
        assert_eq!(issues(&w), Vec::<String>::new(), "{source}/{event}");
    }
}

#[test]
fn an_event_project_is_a_path_or_a_declared_project_input() {
    let w = workflow(
        r#"{"id":"start","type":"event","source":"github","event":"issue_opened","project":"{{inputs.nope}}"}"#,
        "",
    );
    assert!(issues(&w)
        .iter()
        .any(|m| m.contains("not a declared project input")));
    let w = workflow(
        r#"{"id":"start","type":"event","source":"github","event":"issue_opened","project":"{{run.id}}"}"#,
        "",
    );
    assert!(issues(&w)
        .iter()
        .any(|m| m.contains("must be a project path")));
}

#[test]
fn a_code_host_trigger_needs_a_project_but_a_tracker_does_not() {
    let github = r#"{"id":"start","type":"event","source":"github","event":"issue_opened"}"#;
    let mut w = workflow(github, "");
    w.workspace.project.clear();
    assert!(issues(&w).iter().any(|m| m.contains("project is not set")));
    // The workflow's own project is enough.
    w.workspace.project = "/code/app".into();
    assert_eq!(issues(&w), Vec::<String>::new());
    let linear = r#"{"id":"start","type":"event","source":"linear","event":"issue_created"}"#;
    assert_eq!(issues(&workflow(linear, "")), Vec::<String>::new());
}

#[test]
fn an_event_project_must_be_an_app_project() {
    let w = event_workflow("");
    let ok = ["/code/app".to_string()];
    assert!(unknown_project_issues(&w, &ok).is_empty());
    let other = ["/code/other".to_string()];
    let found = unknown_project_issues(&w, &other);
    assert!(
        found.iter().any(|i| i.node_id.as_deref() == Some("start")
            && i.message.contains("not one of the app's projects")),
        "{found:?}"
    );
}

#[test]
fn access_ids_are_checked_and_default_to_auto_accept_edits() {
    for id in ["supervised", "auto-accept-edits", "auto", "full-access"] {
        let w = event_workflow(&format!(r#","access":"{id}""#));
        assert_eq!(issues(&w), Vec::<String>::new(), "{id}");
        assert_eq!(w.node("work").unwrap().kind.session_access(), id);
    }
    for id in ["read-only", "root", ""] {
        let w = event_workflow(&format!(r#","access":"{id}""#));
        assert!(
            issues(&w)
                .iter()
                .any(|m| m.contains("access") && m.contains("is not valid")),
            "{id}"
        );
        // A value the app does not know never widens what the agent may do.
        assert_eq!(
            w.node("work").unwrap().kind.session_access(),
            DEFAULT_SESSION_ACCESS
        );
    }
    let w = event_workflow("");
    assert_eq!(
        w.node("work").unwrap().kind.session_access(),
        "auto-accept-edits"
    );
}

#[test]
fn link_work_item_needs_an_event_trigger_before_the_step() {
    let linked = r#","linkWorkItem":true"#;
    assert_eq!(issues(&event_workflow(linked)), Vec::<String>::new());
    let schedule = r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;
    let w = workflow(schedule, linked);
    assert!(
        issues(&w)
            .iter()
            .any(|m| m.contains("linkWorkItem needs an event trigger")),
        "{:?}",
        issues(&w)
    );
    // Off, or absent, asks for nothing.
    assert_eq!(
        issues(&workflow(schedule, r#","linkWorkItem":false"#)),
        Vec::<String>::new()
    );
    let manual = r#"{"id":"start","type":"manual"}"#;
    assert!(!issues(&workflow(manual, ""))
        .iter()
        .any(|m| m.contains("linkWorkItem")));
}

#[test]
fn a_folder_only_applies_to_a_local_step() {
    let cloud = r#","folder":"fld-1","target":{"kind":"sandbox"},"checkout":"root""#;
    let w = workflow(PR_TRIGGER, cloud);
    assert!(issues(&w)
        .iter()
        .any(|m| m.contains("folder only applies to a local step")));
    assert_eq!(
        issues(&event_workflow(r#","folder":"fld-1""#)),
        Vec::<String>::new()
    );
}

#[test]
fn full_access_downstream_of_an_event_trigger_is_a_warning_not_an_error() {
    let w = event_workflow(r#","access":"full-access""#);
    assert_eq!(issues(&w), Vec::<String>::new());
    let found = warnings(&w);
    assert!(
        found.iter().any(|m| m.contains("full access")
            && m.contains("event trigger")
            && m.contains("\"work\"")),
        "{found:?}"
    );
    // The other levels, and the default, warn about nothing.
    for extra in [
        "",
        r#","access":"auto""#,
        r#","access":"supervised""#,
        r#","access":"auto-accept-edits""#,
    ] {
        assert_eq!(
            warnings(&event_workflow(extra)),
            Vec::<String>::new(),
            "{extra}"
        );
    }
    // Full access after a schedule is the user's own choice: no warning.
    let schedule = r#"{"id":"start","type":"schedule","schedule":{"scheduleKind":"daily","time":"09:00","minute":0,"dayOfWeek":1}}"#;
    assert_eq!(
        warnings(&workflow(schedule, r#","access":"full-access""#)),
        Vec::<String>::new()
    );
}

#[test]
fn full_access_reaches_through_later_steps() {
    let mut w = event_workflow("");
    w.nodes.push(node(
        r#"{"id":"later","type":"session","project":"{{inputs.repo}}","prompt":"go","checkout":"root","outputs":{},"access":"full-access"}"#,
    ));
    w.edges
        .push(super::fixtures::edge(r#"{"from":"work","to":"later"}"#));
    let found = warnings(&w);
    assert_eq!(found.len(), 1, "{found:?}");
    assert!(found[0].contains("\"later\""));
}

fn trigger_item_paths(w: &Workflow) -> Vec<String> {
    available_variables(w, "work")
        .into_iter()
        .map(|v| v.path)
        .filter(|p| p.starts_with("{{trigger.item"))
        .collect()
}

#[test]
fn trigger_item_variables_are_offered_only_with_an_event_trigger() {
    let paths = trigger_item_paths(&event_workflow(""));
    assert_eq!(
        paths,
        [
            "{{trigger.item.source}}",
            "{{trigger.item.kind}}",
            "{{trigger.item.ref}}",
            "{{trigger.item.title}}",
            "{{trigger.item.url}}",
            "{{trigger.item.repo}}",
        ]
    );
    let manual = workflow(r#"{"id":"start","type":"manual"}"#, "");
    assert!(trigger_item_paths(&manual).is_empty());
    // Every offered path validates in the step's own prompt.
    let mut w = event_workflow("");
    let all = available_variables(&w, "work")
        .into_iter()
        .map(|v| v.path)
        .collect::<Vec<_>>()
        .join(" ");
    if let NodeKind::Session { prompt, .. } = &mut w.nodes[1].kind {
        *prompt = all;
    }
    assert_eq!(issues(&w), Vec::<String>::new());
}

#[test]
fn the_fingerprint_covers_the_event_node_and_the_session_fields() {
    let base = definition_fingerprint(&event_workflow(""));
    let mut repos = event_workflow("");
    if let NodeKind::Event(EventTrigger { repos, .. }) = &mut repos.nodes[0].kind {
        repos.push("acme/other".into());
    }
    assert_ne!(definition_fingerprint(&repos), base);
    let mut event = event_workflow("");
    if let NodeKind::Event(e) = &mut event.nodes[0].kind {
        e.event = "issue_opened".into();
    }
    assert_ne!(definition_fingerprint(&event), base);
    for extra in [
        r#","access":"full-access""#,
        r#","folder":"x""#,
        r#","linkWorkItem":true"#,
    ] {
        assert_ne!(
            definition_fingerprint(&event_workflow(extra)),
            base,
            "{extra}"
        );
    }
}

#[test]
fn the_board_can_add_an_event_trigger() {
    use super::board_edit::{picker_groups, picker_unavailable, type_label};
    use super::board_model::{new_node, NodeType};
    let w = event_workflow("");
    let made = new_node(NodeType::Event, &w);
    assert!(made.is_trigger());
    assert!(matches!(made.kind, NodeKind::Event(_)));
    assert_eq!(NodeType::of(&made.kind), NodeType::Event);
    assert_eq!(type_label(NodeType::Event), "Event");
    assert!(picker_groups()
        .iter()
        .any(|g| g.items.iter().any(|(ty, _)| *ty == NodeType::Event)));
    assert!(picker_unavailable(NodeType::Event, 5).is_some());
    assert!(picker_unavailable(NodeType::Event, 1).is_none());
}

#[test]
fn the_guide_and_the_mono_help_describe_the_event_trigger() {
    let guide = super::guide::workflow_guide();
    for point in [
        "`event` (trigger)",
        "`github`/`pull_request_opened`",
        "`azuredevops`/`work_item_appeared`",
        "2 to 5 minutes",
        "only while the app is open",
        "Untrusted input",
        "`access`",
        "`linkWorkItem`",
        "`folder`",
        "{{trigger.item.title}}",
    ] {
        assert!(guide.contains(point), "{point}");
    }
    assert!(crate::mono::bridge::HELP.contains("manual or event"));
}

#[test]
fn the_mono_tools_summarize_event_triggers() {
    let w = event_workflow("");
    let text = crate::mono::workflows::cli::workflow_projects(&w).0;
    // The trigger's project counts as a project of the workflow.
    assert_eq!(text, ["/code/app"]);
    assert_eq!(
        super::event::EventTrigger {
            source: "github".into(),
            event: "pull_request_opened".into(),
            ..Default::default()
        }
        .summary(),
        "GitHub: Pull request opened"
    );
}
