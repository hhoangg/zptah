use super::fixtures::{example, node, patch_node};
use super::model::{NodeKind, Workflow};
use super::validate::{validate_workflow, RUN_VARS};
use super::variables::*;

fn paths(w: &Workflow, id: &str) -> Vec<String> {
    available_variables(w, id)
        .into_iter()
        .map(|v| v.path)
        .collect()
}

fn in_group(w: &Workflow, id: &str, group: &VariableGroup) -> Vec<String> {
    available_variables(w, id)
        .into_iter()
        .filter(|v| &v.group == group)
        .map(|v| v.path)
        .collect()
}

fn issues_of(w: &Workflow, id: &str) -> Vec<String> {
    validate_workflow(w)
        .into_iter()
        .filter(|i| i.node_id.as_deref() == Some(id))
        .map(|i| i.message)
        .collect()
}

/// The node's own text, filled with every variable it is offered, validates.
fn assert_all_validate(w: &Workflow, id: &str) {
    let all = paths(w, id).join(" ");
    let text = format!(
        r#"{{"prompt":{0},"command":{0}}}"#,
        serde_json::to_string(&all).unwrap()
    );
    let w = patch_node(w, id, &text);
    assert_eq!(issues_of(&w, id), Vec::<String>::new(), "variables: {all}");
}

#[test]
fn every_offered_variable_validates_in_the_nodes_own_text() {
    let w = example();
    for id in ["fetch", "dev", "review"] {
        assert_all_validate(&w, id);
    }
    let w = super::fixtures::with_command("echo hi");
    assert_all_validate(&w, "lint");
    let w = super::fixtures::workflow_from(super::fixtures::SINK_JSON);
    for id in ["fetch", "build", "inner", "report"] {
        assert_all_validate(&w, id);
    }
}

#[test]
fn a_node_is_offered_trigger_inputs_and_run_variables() {
    let w = example();
    assert_eq!(
        in_group(&w, "fetch", &VariableGroup::Trigger),
        ["{{trigger.node}}", "{{trigger.kind}}"]
    );
    assert_eq!(
        in_group(&w, "fetch", &VariableGroup::Inputs),
        ["{{inputs.project}}", "{{inputs.clickupList}}"]
    );
    assert_eq!(
        in_group(&w, "fetch", &VariableGroup::Run),
        ["{{run.id}}", "{{run.round}}", "{{run.startedAt}}"]
    );
    let input = available_variables(&w, "fetch")
        .into_iter()
        .find(|v| v.path == "{{inputs.clickupList}}")
        .unwrap();
    assert_eq!(input.label, "ClickUp list to pull from");
}

#[test]
fn the_run_table_is_exactly_what_validation_accepts() {
    let w = example();
    let mut offered: Vec<String> = in_group(&w, "fetch", &VariableGroup::Run)
        .into_iter()
        .map(|p| {
            p.trim_start_matches("{{run.")
                .trim_end_matches("}}")
                .to_string()
        })
        .collect();
    let mut accepted: Vec<String> = RUN_VARS.iter().map(|s| s.to_string()).collect();
    offered.sort();
    accepted.sort();
    assert_eq!(offered, accepted);
}

#[test]
fn only_upstream_steps_are_offered_with_their_outputs() {
    let w = example();
    // Nothing is upstream of the first step.
    assert!(!paths(&w, "fetch").iter().any(|p| p.contains("fetch.")
        || p.contains("dev.")
        || p.contains("review.")
        || p.contains("report.")));
    // A later step sees the earlier one, built-ins first, then the declared outputs.
    let fetch = in_group(&w, "each", &VariableGroup::Step("fetch".into()));
    assert!(fetch.contains(&"{{fetch.reply}}".to_string()));
    assert!(fetch.contains(&"{{fetch.worktree}}".to_string()));
    assert!(fetch.contains(&"{{fetch.tickets}}".to_string()));
    // Downstream nodes are absent: `report` is after `each`, `dev` is in its body.
    let each = paths(&w, "each");
    assert!(!each
        .iter()
        .any(|p| p.contains("dev.") || p.contains("report.")));
    // `review` follows `dev` in the body, so `dev` is offered to it and not the other way round
    // except through the loop (the previous round).
    assert!(paths(&w, "review").contains(&"{{dev.pr_url}}".to_string()));
}

#[test]
fn a_loop_body_is_offered_the_previous_rounds_output() {
    let w = example();
    assert!(paths(&w, "dev").contains(&"{{review.verdict}}".to_string()));
}

#[test]
fn a_for_each_body_is_offered_the_loop_variable_and_downstream_the_results() {
    let w = example();
    let item = in_group(&w, "dev", &VariableGroup::Item);
    assert_eq!(item, ["{{item}}", "{{item.field}}"]);
    assert!(in_group(&w, "fetch", &VariableGroup::Item).is_empty());
    assert!(paths(&w, "report").contains(&"{{each.results}}".to_string()));
}

#[test]
fn a_renamed_loop_variable_is_offered_under_its_name() {
    let w = patch_node(&example(), "each", r#"{"as":"ticket"}"#);
    assert_eq!(
        in_group(&w, "dev", &VariableGroup::Item),
        ["{{ticket}}", "{{ticket.field}}"]
    );
    assert_all_validate(&w, "dev");
}

#[test]
fn session_built_ins_follow_the_surface_and_the_cloud_target() {
    let w = super::fixtures::with_command("echo hi");
    let step = |w: &Workflow| in_group(w, "lint", &VariableGroup::Step("fetch".into()));
    assert!(step(&w).contains(&"{{fetch.reply}}".to_string()));

    let terminal = patch_node(&w, "fetch", r#"{"surface":"terminal"}"#);
    assert!(!step(&terminal).contains(&"{{fetch.reply}}".to_string()));
    assert!(step(&terminal).contains(&"{{fetch.commit}}".to_string()));

    let mut cloud = w.clone();
    let target = super::model::SessionTarget {
        kind: "sandbox".into(),
        instance: super::model::TargetInstance::New,
        options: Default::default(),
    };
    for n in &mut cloud.nodes {
        if let NodeKind::Session { target: t, .. } = &mut n.kind {
            if n.id == "fetch" {
                *t = Some(target.clone());
            }
        }
    }
    assert_eq!(
        step(&cloud),
        [
            "{{fetch.reply}}",
            "{{fetch.sessionId}}",
            "{{fetch.tickets}}"
        ]
    );
}

#[test]
fn a_command_step_offers_exit_code_stdout_and_stderr_with_types() {
    let w = super::fixtures::with_command("echo hi");
    let mut w = w;
    w.nodes.push(node(
        r#"{"id":"after","type":"command","project":"{{inputs.project}}","command":"echo"}"#,
    ));
    w.edges
        .push(super::fixtures::edge(r#"{"from":"lint","to":"after"}"#));
    let vars: Vec<(String, VariableKind)> = available_variables(&w, "after")
        .into_iter()
        .filter(|v| v.group == VariableGroup::Step("lint".into()))
        .map(|v| (v.path, v.kind))
        .collect();
    assert_eq!(
        vars,
        [
            ("{{lint.exitCode}}".to_string(), VariableKind::Number),
            ("{{lint.stdout}}".to_string(), VariableKind::Text),
            ("{{lint.stderr}}".to_string(), VariableKind::Text),
        ]
    );
    assert_all_validate(&w, "after");
}

#[test]
fn a_declared_output_carries_its_type_and_a_missing_node_has_no_variables() {
    let w = example();
    let kind = available_variables(&w, "each")
        .into_iter()
        .find(|v| v.path == "{{fetch.tickets}}")
        .unwrap()
        .kind;
    assert_eq!(kind, VariableKind::List);
    assert!(available_variables(&w, "nope").is_empty());
}

#[test]
fn event_item_variables_wait_for_an_event_trigger() {
    // Hook: no node kind is an event trigger yet, so none is offered.
    let w = example();
    assert!(!paths(&w, "fetch")
        .iter()
        .any(|p| p.starts_with("{{trigger.item")));
}
