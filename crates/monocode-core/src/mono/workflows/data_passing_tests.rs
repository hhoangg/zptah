//! Session `mode`, `surface` and `outputs`: reading, writing, validation, the review text and
//! the definition fingerprint that gates a Mono's changes.
use super::fixtures::{json, workflow_from};
use super::model::*;
use super::ordered::Json;
use super::validate::{validate_workflow, validate_workflow_on};
use super::yaml::{export_workflow, preview_import};
use serde_json::{json as value, Value};

fn session(id: &str, extra: Value) -> Value {
    let mut v = value!({
        "id": id, "type": "session", "project": "{{inputs.project}}", "prompt": "go",
        "checkout": "root", "outputs": {"verdict": {"type": "string"}}
    });
    for (key, field) in extra.as_object().unwrap() {
        v[key] = field.clone();
    }
    v
}

fn command(id: &str, text: &str) -> Value {
    value!({"id": id, "type": "command", "project": "{{inputs.project}}", "command": text})
}

/// trigger -> each node, as a stored workflow.
fn chain(middle: Vec<Value>) -> Workflow {
    let mut ids = vec!["start".to_string()];
    ids.extend(middle.iter().map(|n| n["id"].as_str().unwrap().to_string()));
    let edges: Vec<Value> = ids
        .windows(2)
        .map(|pair| value!({"from": pair[0], "to": pair[1]}))
        .collect();
    let mut nodes = vec![value!({"id": "start", "type": "manual"})];
    nodes.extend(middle);
    workflow_from(
        &value!({
            "id": "wf-abcd1234", "name": "Data", "inputs": {"project": {"type": "project"}},
            "inputValues": {"project": "/work/api"},
            "nodes": nodes, "edges": edges, "enabled": false, "createdAt": 0
        })
        .to_string(),
    )
}

fn messages(w: &Workflow, windows: bool) -> Vec<String> {
    validate_workflow_on(w, windows)
        .into_iter()
        .map(|i| i.message)
        .collect()
}

fn has(w: &Workflow, phrase: &str) -> bool {
    messages(w, false).iter().any(|m| m.contains(phrase))
}

fn fields(w: &Workflow, id: &str) -> (Option<String>, Option<String>) {
    match &w.node(id).unwrap().kind {
        NodeKind::Session { mode, surface, .. } => (mode.clone(), surface.clone()),
        _ => panic!("not a session"),
    }
}

// --- reading and writing ----------------------------------------------------------------

#[test]
fn a_session_defaults_to_auto_and_chat_and_writes_neither() {
    let w = chain(vec![session("dev", value!({}))]);
    assert_eq!(fields(&w, "dev"), (None, None));
    let kind = &w.node("dev").unwrap().kind;
    assert_eq!(kind.session_mode(), SessionMode::Auto);
    assert_eq!(kind.session_surface(), SessionSurface::Chat);
    let written = serde_json::to_value(w.node("dev").unwrap()).unwrap();
    assert!(written.get("mode").is_none() && written.get("surface").is_none());
}

#[test]
fn outputs_is_written_and_output_is_still_read() {
    let old = chain(vec![session("dev", value!({}))]);
    let only_old = workflow_from(&{
        let mut raw = serde_json::to_value(&old).unwrap();
        let nodes = raw["nodes"].as_array_mut().unwrap();
        let dev = nodes[1].as_object_mut().unwrap();
        dev.remove("outputs");
        dev.insert("output".into(), value!({"a": {"type": "number"}}));
        raw.to_string()
    });
    let NodeKind::Session { output, .. } = &only_old.node("dev").unwrap().kind else {
        panic!()
    };
    assert!(output.contains_key("a"));
    let written = serde_json::to_value(only_old.node("dev").unwrap()).unwrap();
    assert!(written.get("outputs").is_some() && written.get("output").is_none());
}

#[test]
fn mode_and_surface_round_trip_through_json_and_yaml() {
    let w = chain(vec![session(
        "dev",
        value!({"mode": "waitForMe", "surface": "terminal", "harness": "claude"}),
    )]);
    assert_eq!(
        fields(&w, "dev"),
        (Some("waitForMe".into()), Some("terminal".into()))
    );
    let again = workflow_from(&serde_json::to_string(&w).unwrap());
    assert_eq!(again.nodes, w.nodes);
    let text = export_workflow(&w);
    assert!(text.contains("surface: terminal") && text.contains("mode: waitForMe"));
    assert!(text.contains("outputs:") && !text.contains("\n    output:"));
    let preview = preview_import(&text, &super::models::ModelCatalog::unprobed()).unwrap();
    assert_eq!(preview.workflow.nodes, w.nodes);
}

#[test]
fn surface_maps_to_the_core_surface() {
    assert_eq!(SessionSurface::Chat.core(), crate::surface::Surface::Chat);
    assert_eq!(
        SessionSurface::Terminal.core(),
        crate::surface::Surface::Tui
    );
    assert_eq!(
        SessionSurface::parse(Some("terminal")),
        Some(SessionSurface::Terminal)
    );
    assert_eq!(SessionSurface::parse(Some("tui")), None);
    assert_eq!(
        SessionMode::parse(Some("waitForMe")),
        Some(SessionMode::WaitForMe)
    );
    assert_eq!(SessionMode::parse(Some("wait")), None);
}

// --- validation -------------------------------------------------------------------------

#[test]
fn a_declared_field_may_not_shadow_a_built_in() {
    for name in [
        "reply",
        "sessionId",
        "worktree",
        "branch",
        "commit",
        "changedFiles",
        "diffStat",
        "exitCode",
        "stdout",
        "stderr",
    ] {
        let w = chain(vec![session(
            "dev",
            value!({"outputs": {name: {"type": "string"}}}),
        )]);
        assert!(
            has(&w, &format!("Output field \"{name}\" is a built-in output")),
            "{name}"
        );
    }
    let ok = chain(vec![session(
        "dev",
        value!({"outputs": {"verdict": {"type": "string"}}}),
    )]);
    assert_eq!(messages(&ok, false), Vec::<String>::new());
    // The reader leaves out a field whose name is not valid; one that got in is reported.
    let mut bad = ok.clone();
    if let NodeKind::Session { output, .. } = &mut bad.nodes[1].kind {
        output.insert("not valid", OutputField::new(OutputFieldType::String));
    }
    assert!(has(&bad, "Invalid output field name"));
}

#[test]
fn built_in_outputs_are_usable_in_templates_and_conditions() {
    let w = workflow_from(
        &value!({
            "id": "wf-abcd1234", "name": "Data", "inputs": {"project": {"type": "project"}},
            "inputValues": {"project": "/work/api"},
            "nodes": [
                {"id": "start", "type": "manual"},
                session("dev", value!({})),
                {"id": "gate", "type": "if", "condition": "{{dev.commit}} != \"\""},
                command("show", "echo {{dev.branch}} {{dev.changedFiles}}"),
                session("review", value!({"prompt": "Review {{dev.reply}} on {{dev.branch}} ({{dev.diffStat}}, {{dev.sessionId}}, {{dev.worktree}})"})),
                command("report", "echo {{show.exitCode}} {{show.stdout}} {{show.stderr}}"),
            ],
            "edges": [
                {"from": "start", "to": "dev"}, {"from": "dev", "to": "gate"},
                {"from": "gate", "to": "show", "handle": "true"},
                {"from": "show", "to": "review"}, {"from": "review", "to": "report"},
            ],
            "enabled": false, "createdAt": 0
        })
        .to_string(),
    );
    assert_eq!(messages(&w, false), Vec::<String>::new());
    let unknown = chain(vec![
        session("dev", value!({})),
        session("next", value!({"prompt": "{{dev.nothing}}"})),
    ]);
    assert!(has(&unknown, "\"dev\" has no output field \"nothing\""));
}

#[test]
fn reply_exists_only_on_chat_nodes() {
    let w = chain(vec![
        session("dev", value!({"surface": "terminal", "harness": "claude"})),
        session("next", value!({"prompt": "{{dev.reply}}"})),
    ]);
    assert!(has(
        &w,
        "\"dev\" has no output field \"reply\" (only a chat session has a reply)"
    ));
    let ok = chain(vec![
        session("dev", value!({"surface": "terminal", "harness": "claude"})),
        session("next", value!({"prompt": "{{dev.branch}} {{dev.verdict}}"})),
    ]);
    assert_eq!(messages(&ok, false), Vec::<String>::new());
}

#[test]
fn a_terminal_step_needs_claude_a_local_project_and_not_windows() {
    let terminal = |extra: Value| {
        let mut v = value!({"surface": "terminal"});
        for (k, x) in extra.as_object().unwrap() {
            v[k] = x.clone();
        }
        chain(vec![session("dev", v)])
    };
    assert_eq!(messages(&terminal(value!({})), false), Vec::<String>::new());
    assert_eq!(
        messages(&terminal(value!({"harness": "claude"})), false),
        Vec::<String>::new()
    );
    assert!(has(
        &terminal(value!({"harness": "codex"})),
        "a terminal step runs Claude only"
    ));
    assert!(has(
        &terminal(value!({"model": "codex:gpt-5"})),
        "a terminal step runs Claude only, but the model belongs to \"codex\""
    ));
    assert!(has(
        &terminal(value!({"target": {"kind": "codespace"}})),
        "cannot have a cloud target"
    ));
    let on_windows = messages(&terminal(value!({})), true);
    assert!(on_windows
        .iter()
        .any(|m| m.contains("not available on Windows")));
    // A chat step has no such limit on any platform.
    let chat = chain(vec![session("dev", value!({"harness": "codex"}))]);
    assert_eq!(messages(&chat, true), Vec::<String>::new());
}

#[test]
fn a_terminal_step_on_a_remote_project_is_refused_and_a_chat_step_is_not() {
    let mut terminal = chain(vec![session("dev", value!({"surface": "terminal"}))]);
    terminal.input_values.insert(
        "project",
        InputValue::String("remote://env/workspaces/app".into()),
    );
    assert!(has(&terminal, "its project is on another machine"));
    let mut chat = chain(vec![session("dev", value!({}))]);
    chat.input_values.insert(
        "project",
        InputValue::String("remote://env/workspaces/app".into()),
    );
    assert_eq!(messages(&chat, false), Vec::<String>::new());
}

#[test]
fn mode_and_surface_values_are_checked() {
    assert!(has(
        &chain(vec![session("dev", value!({"mode": "later"}))]),
        "mode \"later\" is not valid"
    ));
    assert!(has(
        &chain(vec![session("dev", value!({"surface": "tui"}))]),
        "surface \"tui\" is not valid"
    ));
    let ok = chain(vec![session(
        "dev",
        value!({"mode": "waitForMe", "surface": "chat"}),
    )]);
    assert_eq!(messages(&ok, false), Vec::<String>::new());
    // The default platform check is the host's.
    assert_eq!(validate_workflow(&ok), messages_default(&ok));
}

fn messages_default(w: &Workflow) -> Vec<super::validate::WorkflowIssue> {
    validate_workflow_on(w, cfg!(windows))
}

#[test]
fn a_cloud_step_has_only_reply_and_session_id_built_ins() {
    let w = chain(vec![
        session("dev", value!({"target": {"kind": "codespace"}})),
        session(
            "next",
            value!({"prompt": "{{dev.reply}} {{dev.sessionId}} {{dev.verdict}}"}),
        ),
    ]);
    assert_eq!(messages(&w, false), Vec::<String>::new());
    for field in ["worktree", "branch", "commit", "changedFiles", "diffStat"] {
        let bad = chain(vec![
            session("dev", value!({"target": {"kind": "codespace"}})),
            session("next", value!({"prompt": format!("{{{{dev.{field}}}}}")})),
        ]);
        assert!(
            has(&bad, &format!("\"dev\" has no output field \"{field}\" (a cloud step has only the reply and sessionId built-ins)")),
            "{field}"
        );
    }
}

#[test]
fn a_terminal_step_never_reuses_its_session() {
    use super::engine::{next_actions, start_run, EngineAction};
    let w = chain(vec![
        session("dev", value!({"surface": "terminal", "reuseSession": true})),
        session(
            "review",
            value!({"outputs": {"verdict": {"type": "string"}}}),
        ),
    ]);
    // A plain run offers dev with no session to continue.
    let run = start_run(&w, RunCaller::new(CallerKind::Manual, "start"), 1, "r");
    let first = next_actions(&run);
    assert!(matches!(
        first.as_slice(),
        [EngineAction::StartSession {
            reuse_session_id: None,
            ..
        }]
    ));
}

// --- the prompt a reviewer reads and the fingerprint ---------------------------------------

#[test]
fn review_texts_show_the_hint_the_agent_will_read_and_no_json_contract() {
    let w = chain(vec![session("dev", value!({"prompt": "Review it"}))]);
    let (_, prompts) = super::yaml::review_texts(&w.nodes);
    let text = &prompts[0].text;
    assert!(text.starts_with("Review it\n\n"));
    assert!(text.contains("\nRun: zptah app workflow.output '{\"verdict\": \"...\"}'\n"));
    assert!(!text.contains("app app"), "{text}");
    assert!(!text.contains("fenced"));
}

#[test]
fn the_fingerprint_covers_mode_surface_and_outputs() {
    let base = chain(vec![session("dev", value!({}))]);
    let print = definition_fingerprint(&base);
    for patch in [
        value!({"mode": "waitForMe"}),
        value!({"surface": "terminal"}),
        value!({"outputs": {"verdict": {"type": "string"}, "extra": {"type": "list"}}}),
        value!({"outputs": {"verdict": {"type": "number"}}}),
    ] {
        let changed = chain(vec![session("dev", patch.clone())]);
        assert_ne!(definition_fingerprint(&changed), print, "{patch}");
    }
    // The default spelled out is a different file, but the same bytes are the same print.
    assert_eq!(
        definition_fingerprint(&chain(vec![session("dev", value!({}))])),
        print
    );
    // What a pending output holds is run state, not definition.
    let mut moved = base.clone();
    moved.nodes[1].ui = Some(Ui { x: 1.0, y: 2.0 });
    assert_eq!(definition_fingerprint(&moved), print);
}

#[test]
fn a_run_record_keeps_pending_outputs_and_reads_old_records_without_them() {
    let mut r = NodeRun::new("dev", "");
    assert!(serde_json::to_value(&r)
        .unwrap()
        .get("pendingOutput")
        .is_none());
    r.pending_output = Some(match Json::from(&value!({"a": 1})) {
        Json::Object(m) => m,
        _ => unreachable!(),
    });
    let written = serde_json::to_string(&r).unwrap();
    assert!(written.contains("\"pendingOutput\":{\"a\":1}"));
    let back: NodeRun = serde_json::from_str(&written).unwrap();
    assert_eq!(back, r);
    let old: NodeRun =
        serde_json::from_str(r#"{"nodeId":"dev","branch":"","status":"waitingForUser","round":1}"#)
            .unwrap();
    assert_eq!(old.status, NodeStatus::WaitingForUser);
    let _ = json("{}");
}

fn row(role: &str, text: &str, first: bool) -> crate::transcript::Row {
    crate::transcript::Row {
        role: role.into(),
        text: text.into(),
        first,
        sent_at: None,
    }
}

#[test]
fn the_extraction_transcript_labels_messages_and_drops_tools() {
    use super::output::transcript_text;
    let rows = vec![
        row("user", "Fix it", true),
        row("tool", "ls output", true),
        row("reasoning", "hmm", true),
        row("assistant", "Done, ", true),
        row("assistant", "approved.", false),
        row("user", "Thanks", true),
    ];
    assert_eq!(
        transcript_text(&rows, 1_000),
        "User: Fix it\n\nAssistant: Done, approved.\n\nUser: Thanks"
    );
    assert_eq!(transcript_text(&[], 10), "");
}

#[test]
fn a_long_extraction_transcript_keeps_its_start_and_its_end() {
    use super::output::transcript_text;
    let rows = vec![
        row("user", &format!("START{}", "m".repeat(5_000)), true),
        row("assistant", &format!("{}END", "n".repeat(5_000)), true),
    ];
    let text = transcript_text(&rows, 1_000);
    assert!(text.starts_with("User: START"));
    assert!(text.ends_with("END"));
    assert!(text.contains("left out"));
    assert!(text.chars().count() < 1_200);
}

#[test]
fn a_workflow_written_with_the_old_output_key_still_parses_and_is_written_as_outputs() {
    let w = workflow_from(
        &value!({
            "id": "wf-abcd1234", "name": "Old", "inputs": {"project": {"type": "project"}},
            "inputValues": {"project": "/work/api"},
            "nodes": [
                {"id": "start", "type": "manual"},
                {"id": "dev", "type": "session", "project": "{{inputs.project}}", "prompt": "Go",
                 "checkout": "root", "output": {"verdict": {"type": "string"}}},
            ],
            "edges": [{"from": "start", "to": "dev"}],
            "enabled": false, "createdAt": 0
        })
        .to_string(),
    );
    let NodeKind::Session { output, .. } = &w.nodes[1].kind else {
        panic!("not a session")
    };
    assert_eq!(output.keys().cloned().collect::<Vec<_>>(), ["verdict"]);
    let written = serde_json::to_value(&w).unwrap();
    assert!(written["nodes"][1].get("outputs").is_some(), "{written}");
    assert!(written["nodes"][1].get("output").is_none(), "{written}");
}
