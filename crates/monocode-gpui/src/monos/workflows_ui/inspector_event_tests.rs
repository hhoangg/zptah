//! The event inspector: the source and event pickers, the filters, the latest matching items and
//! the test run.
use super::board::{BoardField, BoardPick};
use super::board_tests::*;
use super::graph::GraphEvent;
use super::inspector_event::{EventSources, LATENCY, SOURCES};
use super::tests::{build, history, settle};
use crate::monos::tests::{Setup, setup};
use gpui::{TestAppContext, VisualTestContext};
use monocode_core::{
    inbox::Item,
    mono::workflows::{
        event::events_for,
        model::{CallerKind, NodeKind, Workflow},
    },
};
use serde_json::{Value, json};
use std::{cell::Cell, collections::HashMap, rc::Rc};

const W: &str = "wf-events02";

/// event -> session. The trigger listens for GitHub pull requests in the test project.
pub(super) fn flow(s: &Setup, extra: Value, session: Value) -> Workflow {
    let mut all = json!({
        "id": W, "name": "On new PR", "workspace": {"mode": "none"}, "ui": {"direction": "lr"},
    });
    for (key, value) in extra.as_object().unwrap() {
        all[key] = value.clone();
    }
    let mut dev = json!({
        "id": "dev", "type": "session", "project": s.project, "prompt": "Review it",
        "checkout": "root", "ui": {"x": 336, "y": 42},
    });
    for (key, value) in session.as_object().unwrap() {
        dev[key] = value.clone();
    }
    build(
        all,
        json!([
            {"id": "start", "type": "event", "source": "github", "event": "pull_request_opened",
             "project": s.project, "ui": {"x": 16, "y": 50}},
            dev,
        ]),
        json!([{"from": "start", "to": "dev"}]),
    )
}

pub(super) fn item(s: &Setup, number: i64, minute: u32) -> Item {
    Item {
        provider: "github".into(),
        kind: "pr".into(),
        number,
        repo: "acme/app".into(),
        project_path: s.project.clone(),
        title: format!("Fix {number}"),
        url: format!("https://github.com/acme/app/pull/{number}"),
        updated_at: format!("2026-11-28T10:{minute:02}:00Z"),
        ..Default::default()
    }
}

fn connected(sources: &[&str]) -> HashMap<String, (bool, String)> {
    SOURCES
        .iter()
        .map(|(id, _)| (id.to_string(), (sources.contains(id), String::new())))
        .collect()
}

fn feed(s: &Setup, visual: &mut VisualTestContext, on: &[&str], items: Vec<Item>) {
    let connections = connected(on);
    do_with(s, visual, |m, cx| {
        m.set_event_sources(connections, items, cx)
    });
    settle(s, visual);
}

pub(super) fn open_event(
    s: &Setup,
    w: &Workflow,
    on: &[&str],
    items: Vec<Item>,
    cx: &mut TestAppContext,
) -> VisualTestContext {
    let mut visual = open_board(s, w, cx);
    feed(s, &mut visual, on, items);
    event(s, &mut visual, GraphEvent::Click("start".into()));
    settle(s, &mut visual);
    visual
}

fn trigger_of(
    s: &Setup,
    visual: &mut VisualTestContext,
) -> monocode_core::mono::workflows::event::EventTrigger {
    let NodeKind::Event(trigger) = node_of(s, visual, "start").kind else {
        panic!("an event trigger")
    };
    trigger
}

fn notice_of(s: &Setup, visual: &mut VisualTestContext) -> Option<String> {
    s.manager
        .read_with(visual, |m, _| m.wf.events.notice.clone().map(|(_, t)| t))
}

#[test]
fn connected_sources_come_first_and_the_others_say_they_are_not_connected() {
    let mut sources = EventSources::default();
    sources.connections = connected(&["linear", "github"]);
    let options = sources.source_options("github");
    let ids: Vec<&str> = options.iter().map(|(id, _)| id.as_str()).collect();
    assert_eq!(
        ids,
        [
            "github",
            "linear",
            "gitlab",
            "jira",
            "clickup",
            "azuredevops"
        ]
    );
    assert_eq!(options[0].1, "GitHub");
    assert_eq!(options[1].1, "Linear");
    assert_eq!(options[2].1, "GitLab (not connected)");
    assert_eq!(options[5].1, "Azure DevOps (not connected)");
    // A source the trigger names but this list does not know stays selectable.
    assert!(
        sources
            .source_options("gitea")
            .iter()
            .any(|(id, _)| id == "gitea")
    );
}

#[test]
fn every_source_offers_its_events() {
    for (source, expected) in [
        (
            "github",
            vec!["draft_opened", "pull_request_opened", "issue_opened"],
        ),
        ("gitlab", vec!["merge_request_opened", "issue_opened"]),
        ("linear", vec!["issue_created"]),
        ("jira", vec!["issue_created"]),
        ("clickup", vec!["issue_created"]),
        (
            "azuredevops",
            vec!["pull_request_appeared", "work_item_appeared"],
        ),
    ] {
        let ids: Vec<&str> = events_for(source).into_iter().map(|(id, _)| id).collect();
        assert_eq!(ids, expected, "{source}");
    }
}

#[gpui::test]
fn an_unconnected_source_has_a_connect_link_that_opens_the_inbox_settings(cx: &mut TestAppContext) {
    let s = setup(cx);
    let opened = Rc::new(Cell::new(0));
    let seen = opened.clone();
    let _subscription = cx.update(|cx| {
        cx.subscribe(
            &s.manager,
            move |_, _: &crate::monos::OpenInboxSettings, _| seen.set(seen.get() + 1),
        )
    });
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["linear"], Vec::new(), cx);
    let visual = &mut visual;
    // GitHub is not connected here.
    assert!(visual.debug_bounds("wf-event-connect").is_some());
    click_in_inspector(visual, "wf-event-connect");
    assert_eq!(opened.get(), 1);
    // Once it is connected the link goes.
    feed(&s, visual, &["github"], Vec::new());
    assert!(visual.debug_bounds("wf-event-connect").is_none());
}

#[gpui::test]
fn the_source_and_event_pickers_write_the_trigger(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github", "gitlab"], Vec::new(), cx);
    let visual = &mut visual;
    assert!(
        visual
            .debug_bounds("wf-pick:EventSource(\"start\")")
            .is_some()
    );
    assert!(
        visual
            .debug_bounds("wf-pick:EventKind(\"start\")")
            .is_some()
    );
    pick(
        &s,
        visual,
        BoardPick::EventKind("start".into()),
        "issue_opened",
    );
    assert_eq!(trigger_of(&s, visual).event, "issue_opened");
    type_in(
        &s,
        visual,
        BoardField::EventRepos("start".into()),
        "acme/app",
    );
    pick(&s, visual, BoardPick::EventSource("start".into()), "gitlab");
    let trigger = trigger_of(&s, visual);
    // The events and the repositories of the other source do not carry over.
    assert_eq!(trigger.source, "gitlab");
    assert_eq!(trigger.event, "merge_request_opened");
    assert!(trigger.repos.is_empty());
}

#[gpui::test]
fn the_filters_write_and_save(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    // The workspace project is the default: no project of its own.
    pick(&s, visual, BoardPick::EventProject("start".into()), "");
    assert_eq!(trigger_of(&s, visual).project, None);
    pick(
        &s,
        visual,
        BoardPick::EventProject("start".into()),
        &s.project,
    );
    assert_eq!(
        trigger_of(&s, visual).project.as_deref(),
        Some(s.project.as_str())
    );
    type_in(
        &s,
        visual,
        BoardField::EventRepos("start".into()),
        "acme/app, acme/web ,",
    );
    assert_eq!(trigger_of(&s, visual).repos, ["acme/app", "acme/web"]);
    assert!(can_save(&s, visual));
    do_with(&s, visual, |m, cx| m.board_save(false, false, false, cx));
    settle(&s, visual);
    let NodeKind::Event(saved) = stored_first(&s)
        .nodes
        .into_iter()
        .find(|n| n.id == "start")
        .unwrap()
        .kind
    else {
        panic!()
    };
    assert_eq!(saved.repos, ["acme/app", "acme/web"]);
    assert_eq!(saved.project.as_deref(), Some(s.project.as_str()));
}

#[gpui::test]
fn a_clickup_trigger_picks_a_space_instead_of_typing_repositories(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github", "clickup"], Vec::new(), cx);
    let visual = &mut visual;
    assert!(
        visual
            .debug_bounds("wf-pick:EventSpace(\"start\")")
            .is_none()
    );
    pick(
        &s,
        visual,
        BoardPick::EventSource("start".into()),
        "clickup",
    );
    settle(&s, visual);
    assert!(
        visual
            .debug_bounds("wf-pick:EventSpace(\"start\")")
            .is_some()
    );
    assert!(visual.debug_bounds("wf-b:EventRepos(\"start\")").is_none());
    pick(&s, visual, BoardPick::EventSpace("start".into()), "901");
    assert_eq!(trigger_of(&s, visual).repos, ["901"]);
    pick(&s, visual, BoardPick::EventSpace("start".into()), "");
    assert!(trigger_of(&s, visual).repos.is_empty());
}

#[gpui::test]
fn the_inspector_says_when_events_arrive(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    assert!(visual.debug_bounds("wf-event-latency").is_some());
    assert_eq!(
        LATENCY,
        "Events arrive 2-5 minutes after they appear, and only while zptah is open."
    );
}

#[gpui::test]
fn the_three_latest_matching_items_are_listed(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut other = item(&s, 99, 59);
    other.provider = "gitlab".into();
    let mut elsewhere = item(&s, 98, 58);
    elsewhere.project_path = "/somewhere/else".into();
    // Newest first, as the Inbox holds them.
    let items = vec![
        other,
        elsewhere,
        item(&s, 5, 50),
        item(&s, 4, 40),
        item(&s, 3, 30),
        item(&s, 2, 20),
    ];
    let mut visual = open_event(&s, &w, &["github"], items, cx);
    let visual = &mut visual;
    for at in 0..3 {
        let selector: &'static str = ["wf-event-item:0", "wf-event-item:1", "wf-event-item:2"][at];
        assert!(visual.debug_bounds(selector).is_some(), "{selector}");
    }
    assert!(visual.debug_bounds("wf-event-item:3").is_none());
    assert!(visual.debug_bounds("wf-event-no-items").is_none());
    let shown: Vec<i64> = s.manager.read_with(visual, |m, _| {
        super::inspector_event::latest_matching(
            &m.wf.board.as_ref().unwrap().current(),
            &match &m.wf.board.as_ref().unwrap().node("start").unwrap().kind {
                NodeKind::Event(t) => t.clone(),
                _ => unreachable!(),
            },
            std::slice::from_ref(&s.project),
            &m.wf.events.items,
        )
        .iter()
        .map(|i| i.number)
        .collect()
    });
    assert_eq!(shown, [5, 4, 3]);
}

#[gpui::test]
fn with_no_matching_item_the_inspector_says_so(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    assert!(visual.debug_bounds("wf-event-no-items").is_some());
    assert!(visual.debug_bounds("wf-event-test:0").is_none());
}

#[gpui::test]
fn a_test_run_starts_a_manual_run_with_the_item(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], vec![item(&s, 7, 10)], cx);
    let visual = &mut visual;
    click_in_inspector(visual, "wf-event-test:0");
    settle(&s, visual);
    let runs = history(&s, W);
    assert_eq!(runs.len(), 1);
    assert_eq!(runs[0].caller.kind, CallerKind::Manual);
    assert_eq!(runs[0].caller.trigger_node, "start");
    let item = runs[0].caller.item.as_ref().expect("the item");
    assert_eq!(item.get("ref").and_then(|v| v.as_str()), Some("#7"));
    assert!(notice_of(&s, visual).is_some_and(|text| text.contains("Started a test run")));
    // No claim was taken: the same item still fires the trigger later.
    let claims = monocode_core::mono::workflows::event::event_key(&item_of(&s));
    assert!(
        crate::runtime::locked(&s.store)
            .unwrap()
            .claim_workflow_item(W, "start", &claims)
            .unwrap()
    );
}

fn item_of(s: &Setup) -> Item {
    item(s, 7, 10)
}

#[gpui::test]
fn a_test_run_is_allowed_while_the_workflow_is_paused(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({"enabled": false}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], vec![item(&s, 7, 10)], cx);
    let visual = &mut visual;
    click_in_inspector(visual, "wf-event-test:0");
    settle(&s, visual);
    assert_eq!(history(&s, W).len(), 1);
}

#[gpui::test]
fn a_test_run_is_refused_while_the_workflow_needs_review(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(
        &s,
        json!({"enabled": false, "needsReview": true}),
        json!({}),
    );
    let mut visual = open_event(&s, &w, &["github"], vec![item(&s, 7, 10)], cx);
    let visual = &mut visual;
    click_in_inspector(visual, "wf-event-test:0");
    settle(&s, visual);
    assert!(history(&s, W).is_empty());
    let notice = notice_of(&s, visual).expect("a notice");
    assert!(notice.contains("Approve the workflow"), "{notice}");
    assert!(visual.debug_bounds("wf-event-notice").is_some());
}

#[gpui::test]
fn a_test_run_asks_to_save_a_changed_draft_first(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_event(&s, &w, &["github"], vec![item(&s, 7, 10)], cx);
    let visual = &mut visual;
    type_in(&s, visual, BoardField::Title("start".into()), "Renamed");
    settle(&s, visual);
    click_in_inspector(visual, "wf-event-test:0");
    settle(&s, visual);
    assert!(history(&s, W).is_empty());
    let notice = notice_of(&s, visual).expect("a notice");
    assert!(notice.contains("Save the workflow first"), "{notice}");
}

#[gpui::test]
fn a_step_with_full_access_after_an_event_trigger_shows_the_warning(cx: &mut TestAppContext) {
    let s = setup(cx);
    let w = flow(&s, json!({}), json!({"access": "full-access"}));
    let mut visual = open_event(&s, &w, &["github"], Vec::new(), cx);
    let visual = &mut visual;
    event(&s, visual, GraphEvent::Click("dev".into()));
    settle(&s, visual);
    assert!(visual.debug_bounds("wf-node-warning").is_some());
    // A step that keeps the default access has none.
    let w = flow(&s, json!({}), json!({}));
    let mut visual = open_board(&s, &w, cx);
    event(&s, &mut visual, GraphEvent::Click("dev".into()));
    settle(&s, &mut visual);
    assert!(visual.debug_bounds("wf-node-warning").is_none());
}
