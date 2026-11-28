use super::event::check_event;
use super::model::{NodeKind, WorkspaceMode};
use super::templates::{template, templates, TemplateTrigger};
use super::validate::{unknown_project_issues, validate_workflow_on};

const PROJECT: &str = "/work/app";

#[test]
fn every_template_validates_once_a_project_is_set() {
    assert_eq!(templates().len(), 14);
    for t in templates() {
        let w = t.instantiate(PROJECT, 1);
        for windows in [false, true] {
            let issues = validate_workflow_on(&w, windows);
            assert!(issues.is_empty(), "{}: {issues:?}", t.id);
        }
        let unknown = unknown_project_issues(&w, &[PROJECT.to_string()]);
        assert!(unknown.is_empty(), "{}: {unknown:?}", t.id);
        assert!(!w.enabled, "{} starts paused", t.id);
        assert_eq!(w.workspace.mode, WorkspaceMode::PerRun);
        assert_eq!(w.workspace.project, PROJECT);
        assert_eq!(w.name, t.name);
        assert_eq!(w.description.as_deref(), Some(t.description.as_str()));
    }
}

#[test]
fn template_ids_are_unique_and_scheduled_ones_come_first() {
    let all = templates();
    let mut ids: Vec<_> = all.iter().map(|t| t.id.as_str()).collect();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), all.len());
    let first_event = all.iter().position(|t| !t.is_scheduled()).unwrap();
    assert!(all[first_event..].iter().all(|t| !t.is_scheduled()));
    assert_eq!(all.iter().filter(|t| !t.is_scheduled()).count(), 4);
}

#[test]
fn event_templates_use_valid_pairs_and_accept_edits() {
    for t in templates().iter().filter(|t| !t.is_scheduled()) {
        let TemplateTrigger::Event { source, event } = &t.trigger else {
            unreachable!()
        };
        assert_eq!(check_event(source, event), Ok(()), "{}", t.id);
        let w = t.instantiate(PROJECT, 1);
        assert!(
            matches!(&w.nodes[0].kind, NodeKind::Event(e) if e.source == *source && e.event == *event)
        );
        assert_eq!(w.nodes[1].kind.session_access(), "auto-accept-edits");
        // The default is left out of the file.
        assert!(matches!(
            &w.nodes[1].kind,
            NodeKind::Session { access: None, .. }
        ));
        // The prompt names the item the run is about.
        let NodeKind::Session { prompt, .. } = &w.nodes[1].kind else {
            unreachable!()
        };
        for field in ["{{trigger.item.ref}}", "{{trigger.item.url}}"] {
            assert!(prompt.contains(field), "{}: {field}", t.id);
        }
        assert!(validate_workflow_on(&w, false).is_empty(), "{}", t.id);
    }
    let pr = template("review-pull-requests").unwrap();
    assert_eq!(pr.trigger_label(), "GitHub: Pull request opened");
}

#[test]
fn schedule_templates_map_their_schedules() {
    let bugs = template("find-critical-bugs")
        .unwrap()
        .instantiate(PROJECT, 1);
    let NodeKind::Schedule { schedule } = &bugs.nodes[0].kind else {
        panic!("not a schedule")
    };
    assert_eq!(
        (schedule.schedule_kind.as_str(), schedule.time.as_str()),
        ("weekdays", "09:00")
    );
    let scan = template("scan-vulnerabilities").unwrap();
    assert_eq!(scan.trigger_label(), "Monday at 10:00");
    let changelog = template("weekly-changelog").unwrap();
    assert_eq!(changelog.trigger_label(), "Friday at 16:00");
    let NodeKind::Session { prompt, .. } = &bugs.nodes[1].kind else {
        panic!("not a session")
    };
    assert!(prompt.starts_with("Review recent git history"));
    assert_eq!(bugs.edges.len(), 1);
}
