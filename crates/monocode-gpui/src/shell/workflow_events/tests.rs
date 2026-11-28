//! The Inbox wiring of event triggers: what an appeared item starts, what is retried and when the
//! Inbox polls in the background. Against the real shell over a test database; no provider is
//! ever polled and no step reaches a real CLI.
use super::*;
use crate::monos::tests::{Setup, setup};
use gpui::TestAppContext;
use monocode_core::mono::workflows::{
    global::WorkflowsDir,
    model::Workflow,
    ordered::Json,
    parse::parse_workflow,
    store::{load_runs, update_workflows},
};
use serde_json::json;

const WF: &str = "wf-inbox001";

fn event(id: &str, project: &str) -> serde_json::Value {
    json!({"id": id, "type": "event", "source": "github", "event": "pull_request_opened",
           "project": project})
}

fn workflow(s: &Setup, id: &str, extra: serde_json::Value) -> Workflow {
    let mut def = json!({
        "id": id, "name": "On new PR", "inputs": {}, "inputValues": {},
        "nodes": [
            event("start", &s.project),
            {"id": "dev", "type": "session", "project": s.project, "prompt": "Review it",
             "checkout": "root", "outputs": {}},
        ],
        "edges": [{"from": "start", "to": "dev"}],
        "workspace": {"mode": "none"},
        "enabled": true, "createdAt": 1,
    });
    for (key, value) in extra.as_object().unwrap() {
        def[key] = value.clone();
    }
    parse_workflow(&Json::from(&def)).expect("a workflow")
}

fn dir(s: &Setup, cx: &mut TestAppContext) -> WorkflowsDir {
    s.window
        .update(cx, |shell, _, cx| {
            shell.workflows().expect("workflows").read(cx).dir().clone()
        })
        .unwrap()
}

fn save(s: &Setup, cx: &mut TestAppContext, all: Vec<Workflow>) {
    let dir = dir(s, cx);
    update_workflows(&dir, |_| Ok((all.clone(), ()))).unwrap();
}

fn item(s: &Setup, number: i64) -> Item {
    Item {
        provider: "github".into(),
        kind: "pr".into(),
        number,
        repo: "acme/app".into(),
        project_path: s.project.clone(),
        title: format!("Fix {number}"),
        url: format!("https://github.com/acme/app/pull/{number}"),
        updated_at: "2026-11-28T10:00:00Z".into(),
        ..Default::default()
    }
}

/// The Inbox says these items appeared; the starts are awaited.
fn appear(s: &Setup, cx: &mut TestAppContext, items: Vec<Item>) {
    s.window
        .update(cx, |shell, _, cx| shell.inbox_workflow_events(items, cx))
        .unwrap();
    cx.run_until_parked();
}

fn runs(s: &Setup, cx: &mut TestAppContext, id: &str) -> usize {
    let dir = dir(s, cx);
    load_runs(&dir, id).unwrap().len()
}

fn retries(s: &Setup, cx: &mut TestAppContext) -> usize {
    s.window
        .update(cx, |shell, _, _| shell.workflow_events.retries.len())
        .unwrap()
}

/// Makes the claims table refuse a claim, or accept again.
fn claims_refuse(s: &Setup, refuse: bool) {
    locked(&s.store)
        .unwrap()
        .refuse_workflow_claims(refuse)
        .unwrap();
}

#[gpui::test]
fn an_appeared_item_starts_exactly_one_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 1);
    // A started item is not kept for a retry.
    assert_eq!(retries(&s, cx), 0);
}

#[gpui::test]
fn the_same_item_again_starts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    appear(&s, cx, vec![item(&s, 7)]);
    appear(&s, cx, vec![item(&s, 7)]);
    appear(&s, cx, vec![item(&s, 7), item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 1);
}

#[gpui::test]
fn an_item_that_matches_no_trigger_starts_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    let mut other = item(&s, 9);
    other.provider = "gitlab".into();
    let mut elsewhere = item(&s, 10);
    elsewhere.project_path = "/somewhere/else".into();
    appear(&s, cx, vec![other, elsewhere]);
    assert_eq!(runs(&s, cx, WF), 0);
    assert_eq!(retries(&s, cx), 0);
}

#[gpui::test]
fn a_disabled_or_unreviewed_workflow_does_nothing(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(
        &s,
        cx,
        vec![
            workflow(&s, WF, json!({"enabled": false})),
            workflow(&s, "wf-inbox002", json!({"needsReview": true})),
        ],
    );
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 0);
    assert_eq!(runs(&s, cx, "wf-inbox002"), 0);
    // Nothing was claimed, and the item is not kept: enabling later starts nothing for it.
    assert_eq!(retries(&s, cx), 0);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    appear(&s, cx, Vec::new());
    assert_eq!(runs(&s, cx, WF), 0);
}

#[gpui::test]
fn two_matching_triggers_give_one_run(cx: &mut TestAppContext) {
    let s = setup(cx);
    let two = workflow(
        &s,
        WF,
        json!({
            "nodes": [
                event("first", &s.project),
                event("second", &s.project),
                {"id": "dev", "type": "session", "project": s.project, "prompt": "Review it",
                 "checkout": "root", "outputs": {}},
            ],
            "edges": [{"from": "first", "to": "dev"}, {"from": "second", "to": "dev"}],
        }),
    );
    save(&s, cx, vec![two]);
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 1);
    let dir = dir(&s, cx);
    assert_eq!(
        load_runs(&dir, WF).unwrap()[0].caller.trigger_node,
        "first",
        "the first matching trigger wins"
    );
}

#[gpui::test]
fn two_workflows_each_start_a_run_for_one_item(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(
        &s,
        cx,
        vec![
            workflow(&s, WF, json!({})),
            workflow(&s, "wf-inbox002", json!({})),
        ],
    );
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 1);
    assert_eq!(runs(&s, cx, "wf-inbox002"), 1);
}

#[gpui::test]
fn a_failed_claim_is_retried_and_then_succeeds_once(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    claims_refuse(&s, true);
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 0);
    assert_eq!(retries(&s, cx), 1, "the item waits for another look");
    // The retry is saved, so a restart offers it again.
    let saved = locked(&s.store)
        .unwrap()
        .setting(RETRY_SETTING)
        .unwrap()
        .unwrap();
    assert_eq!(serde_json::from_str::<Vec<Retry>>(&saved).unwrap().len(), 1);
    claims_refuse(&s, false);
    // The next look has no new item: the retry map alone starts the run.
    appear(&s, cx, Vec::new());
    assert_eq!(runs(&s, cx, WF), 1);
    assert_eq!(retries(&s, cx), 0);
    appear(&s, cx, Vec::new());
    appear(&s, cx, vec![item(&s, 7)]);
    assert_eq!(runs(&s, cx, WF), 1);
}

#[gpui::test]
fn a_saved_retry_is_offered_again_after_a_restart(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    claims_refuse(&s, true);
    appear(&s, cx, vec![item(&s, 7)]);
    claims_refuse(&s, false);
    // A new run of the app starts with an empty map and reads the setting.
    s.window
        .update(cx, |shell, _, _| {
            shell.workflow_events = State::default();
        })
        .unwrap();
    assert_eq!(retries(&s, cx), 0);
    appear(&s, cx, Vec::new());
    assert_eq!(runs(&s, cx, WF), 1);
}

#[gpui::test]
fn only_the_pairs_that_failed_are_offered_the_item_again(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    claims_refuse(&s, true);
    appear(&s, cx, vec![item(&s, 7)]);
    let failed = s
        .window
        .update(cx, |shell, _, _| {
            shell
                .workflow_events
                .retries
                .values()
                .map(|r| (r.failed.clone(), r.attempts))
                .collect::<Vec<_>>()
        })
        .unwrap();
    assert_eq!(failed, [(vec![(WF.to_string(), "start".to_string())], 1)]);
    // A workflow made after the item came never sees it, retry or not.
    save(
        &s,
        cx,
        vec![
            workflow(&s, WF, json!({})),
            workflow(&s, "wf-inbox002", json!({})),
        ],
    );
    claims_refuse(&s, false);
    appear(&s, cx, Vec::new());
    assert_eq!(runs(&s, cx, WF), 1);
    assert_eq!(runs(&s, cx, "wf-inbox002"), 0);
}

#[gpui::test]
fn an_item_is_given_up_on_after_five_failed_offers(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    claims_refuse(&s, true);
    appear(&s, cx, vec![item(&s, 7)]);
    for _ in 1..RETRY_ATTEMPTS - 1 {
        appear(&s, cx, Vec::new());
        assert_eq!(retries(&s, cx), 1);
    }
    appear(&s, cx, Vec::new());
    assert_eq!(retries(&s, cx), 0, "dropped after the last attempt");
    claims_refuse(&s, false);
    appear(&s, cx, Vec::new());
    assert_eq!(runs(&s, cx, WF), 0);
}

#[test]
fn the_retry_map_keeps_the_newest_items() {
    let mut map = HashMap::new();
    for number in 0..(RETRY_CAP as i64 + 40) {
        let item = Item {
            provider: "github".into(),
            kind: "pr".into(),
            number,
            repo: "acme/app".into(),
            updated_at: format!("2026-11-28T10:{:02}:{:02}Z", number / 60 % 60, number % 60),
            ..Default::default()
        };
        map.insert(item.key(), Retry::new(item));
    }
    cap_retries(&mut map);
    assert_eq!(map.len(), RETRY_CAP);
    let newest = RETRY_CAP as i64 + 39;
    assert!(map.values().any(|r| r.item.number == newest));
    assert!(!map.values().any(|r| r.item.number == 0));
}

#[test]
fn items_that_existed_before_are_never_reported() {
    use monocode_core::inbox::{NotificationTracker, Snapshot};
    let existing = Item {
        provider: "github".into(),
        kind: "pr".into(),
        number: 1,
        repo: "acme/app".into(),
        updated_at: "2026-11-28T10:00:00Z".into(),
        ..Default::default()
    };
    let snapshot = Snapshot {
        items: vec![existing.clone()],
        ..Default::default()
    };
    let mut tracker = NotificationTracker::default();
    // The first look primes the tracker: the backlog is not an event.
    assert!(tracker.observe(&snapshot, "scope".into()).is_empty());
    assert!(tracker.observe(&snapshot, "scope".into()).is_empty());
    let mut next = snapshot.clone();
    next.items.push(Item {
        number: 2,
        ..existing
    });
    let appeared = tracker.observe(&next, "scope".into());
    assert_eq!(appeared.len(), 1);
    assert_eq!(appeared[0].number, 2);
}

#[gpui::test]
fn the_background_inbox_starts_only_for_an_enabled_event_workflow(cx: &mut TestAppContext) {
    let s = setup(cx);
    let inbox = |s: &Setup, cx: &mut TestAppContext| {
        s.window
            .update(cx, |shell, _, _| shell.inbox.is_some())
            .unwrap()
    };
    let look = |s: &Setup, cx: &mut TestAppContext| {
        s.window
            .update(cx, |shell, _, cx| shell.sync_workflow_event_inbox(cx))
            .unwrap();
        cx.run_until_parked();
    };
    look(&s, cx);
    assert!(!inbox(&s, cx), "no workflow");
    let schedule = workflow(
        &s,
        "wf-sched001",
        json!({
            "nodes": [
                {"id": "start", "type": "manual"},
                {"id": "dev", "type": "session", "project": s.project, "prompt": "Go",
                 "checkout": "root", "outputs": {}},
            ],
        }),
    );
    save(
        &s,
        cx,
        vec![schedule, workflow(&s, WF, json!({"enabled": false}))],
    );
    look(&s, cx);
    assert!(!inbox(&s, cx), "no enabled event workflow");
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    look(&s, cx);
    assert!(inbox(&s, cx), "an enabled event workflow");
}

#[gpui::test]
fn the_inbox_event_reaches_the_workflows(cx: &mut TestAppContext) {
    let s = setup(cx);
    save(&s, cx, vec![workflow(&s, WF, json!({}))]);
    s.window
        .update(cx, |shell, window, cx| {
            shell.ensure_background_inbox(window, cx);
            let inbox = shell.inbox.clone().expect("the inbox");
            let appeared = vec![item(&s, 7)];
            inbox.update(cx, |_, cx| {
                cx.emit(crate::inbox::Event::ItemsChanged(appeared))
            });
        })
        .unwrap();
    cx.run_until_parked();
    assert_eq!(runs(&s, cx, WF), 1);
}

#[gpui::test]
fn opening_the_workflows_page_does_not_start_the_background_inbox(cx: &mut TestAppContext) {
    let s = setup(cx);
    s.window
        .update(cx, |shell, window, cx| shell.open_workflows(window, cx))
        .unwrap();
    cx.run_until_parked();
    assert!(
        s.window
            .update(cx, |shell, _, _| shell.inbox.is_none())
            .unwrap()
    );
}
